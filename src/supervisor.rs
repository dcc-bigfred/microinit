//! Service supervisor: one monitor thread per service, restart with backoff.
//!
//! Child process waits are owned by the central PID 1 reaper ([`ExitRegistry`]),
//! not by `std::process::Child::wait`, to avoid racing `waitpid(-1)`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use nix::unistd::Pid;

use crate::config::{Config, ServiceConfig};
use crate::console::Console;
use crate::constants::{
    BOOT_BG_SETTLE, BOOT_FG_SETTLE, CTL_POLL, DAEMONIZE_GRACE, DEP_WAIT, EVENT_RETURN,
    EVENT_RING_CAP, SHUTDOWN_QUIT_WAIT, SHUTDOWN_STOP_WAIT, STOP_GRACE_SECS,
};
use crate::error::{Error, Result};
use crate::graph::{partition_boot, shutdown_order};
use crate::liveness::{run_probe, ProbeResult};
use crate::logs::{capture_stream, LogHub, INIT_SERVICE};
use crate::protocol::{
    DaemonInfo, DepNode, LogLevel, ServiceDescribe, ServiceEvent, ServiceEventKind, ServiceSource,
    ServiceState, ServiceStatus,
};
use crate::reaper::{ensure_reaper_thread, global_exits, ExitRegistry};
use crate::service::{run_shell, spawn_shell, terminate_pid};
use crate::syncutil::mutex_lock;

/// Internal ring entry: timestamp is `Copy` so the hot `set_state` path
/// does not allocate; RFC3339 formatting happens only in `describe`.
#[derive(Debug, Clone)]
struct RuntimeEvent {
    ts: DateTime<Utc>,
    kind: ServiceEventKind,
    from: Option<ServiceState>,
    to: Option<ServiceState>,
    detail: Option<String>,
}

impl RuntimeEvent {
    fn to_protocol(&self) -> ServiceEvent {
        ServiceEvent {
            ts: self.ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            kind: self.kind,
            from: self.from,
            to: self.to,
            detail: self.detail.clone(),
        }
    }
}

#[derive(Debug)]
struct Runtime {
    state: ServiceState,
    pid: Option<i32>,
    restarts: u32,
    liveness_failures: u32,
    enabled: bool,
    running_since: Option<Instant>,
    events: VecDeque<RuntimeEvent>,
}

fn new_runtime(enabled: bool) -> Runtime {
    Runtime {
        state: if enabled {
            ServiceState::Pending
        } else {
            ServiceState::Disabled
        },
        pid: None,
        restarts: 0,
        liveness_failures: 0,
        enabled,
        running_since: None,
        // Pre-allocate full ring so push never reallocates on the hot path.
        events: VecDeque::with_capacity(EVENT_RING_CAP),
    }
}

fn push_event_into(
    rt: &mut Runtime,
    kind: ServiceEventKind,
    from: Option<ServiceState>,
    to: Option<ServiceState>,
    detail: Option<String>,
) {
    if rt.events.len() >= EVENT_RING_CAP {
        rt.events.pop_front();
    }
    rt.events.push_back(RuntimeEvent {
        ts: Utc::now(),
        kind,
        from,
        to,
        detail,
    });
}

/// Snapshot of a service for metrics exporters.
#[derive(Debug, Clone)]
pub struct ServiceMetrics {
    pub name: String,
    pub restarts: u32,
    pub liveness_failures: u32,
    pub pid: Option<i32>,
    pub enabled: bool,
    pub state: ServiceState,
    pub uptime_secs: f64,
}

/// Shared runtime map + waiters.
///
/// # Lock order
/// When a path must hold both `runtimes` and [`Supervisor::config`], acquire
/// **`runtimes` first, then `config`**. Never reverse that order. Prefer
/// releasing both before heavy work (see [`Supervisor::describe`]).
struct Shared {
    runtimes: Mutex<HashMap<String, Runtime>>,
    cv: Condvar,
    stop_all: AtomicBool,
}

impl Shared {
    fn set_state(&self, name: &str, state: ServiceState, pid: Option<i32>) {
        let mut map = mutex_lock(&self.runtimes);
        if let Some(rt) = map.get_mut(name) {
            let old = rt.state;
            if old != state {
                push_event_into(
                    rt,
                    ServiceEventKind::StateChange,
                    Some(old),
                    Some(state),
                    None,
                );
            }
            rt.state = state;
            match (state, pid) {
                (_, Some(p)) => rt.pid = Some(p),
                (
                    ServiceState::Running
                    | ServiceState::Starting
                    | ServiceState::Restarting
                    | ServiceState::WaitingForDependency,
                    None,
                ) => {
                    // keep existing pid
                }
                _ => rt.pid = None,
            }
            // Uptime tracks the current continuous `Running` period only.
            match state {
                ServiceState::Running => {
                    if rt.running_since.is_none() {
                        rt.running_since = Some(Instant::now());
                    }
                }
                _ => rt.running_since = None,
            }
        }
        self.cv.notify_all();
    }

    fn bump_restarts(&self, name: &str) {
        if let Some(rt) = mutex_lock(&self.runtimes).get_mut(name) {
            rt.restarts = rt.restarts.saturating_add(1);
            push_event_into(rt, ServiceEventKind::Restart, None, None, None);
        }
    }

    fn bump_liveness_failures(&self, name: &str, detail: Option<String>) {
        if let Some(rt) = mutex_lock(&self.runtimes).get_mut(name) {
            rt.liveness_failures = rt.liveness_failures.saturating_add(1);
            push_event_into(rt, ServiceEventKind::LivenessFailed, None, None, detail);
        }
    }

    /// Flip the enabled flag, then transition state via [`Self::set_state`]
    /// so lifecycle events and `running_since` stay consistent.
    fn set_enabled(&self, name: &str, enabled: bool) {
        let next = {
            let mut map = mutex_lock(&self.runtimes);
            let Some(rt) = map.get_mut(name) else {
                return;
            };
            rt.enabled = enabled;
            if !enabled {
                Some(ServiceState::Disabled)
            } else if matches!(rt.state, ServiceState::Disabled) {
                Some(ServiceState::Pending)
            } else {
                None
            }
        };
        if let Some(state) = next {
            self.set_state(name, state, None);
        } else {
            self.cv.notify_all();
        }
    }

    fn is_enabled(&self, name: &str) -> Result<bool> {
        mutex_lock(&self.runtimes)
            .get(name)
            .map(|r| r.enabled)
            .ok_or_else(|| Error::UnknownService(name.to_string()))
    }

    fn current_state(&self, name: &str) -> Option<ServiceState> {
        mutex_lock(&self.runtimes).get(name).map(|r| r.state)
    }

    /// Names of dependencies that are not yet `Running`/`Succeeded`.
    fn unmet_deps(&self, deps: &[String]) -> Result<Vec<String>> {
        let map = mutex_lock(&self.runtimes);
        let mut out = Vec::new();
        for dep in deps {
            let Some(rt) = map.get(dep) else {
                return Err(Error::UnknownService(dep.clone()));
            };
            if !matches!(rt.state, ServiceState::Running | ServiceState::Succeeded) {
                out.push(dep.clone());
            }
        }
        Ok(out)
    }

    /// `Ok(true)` when every dependency is `Running` or `Succeeded`.
    /// Missing deps → error. Not-yet-ready (including `Failed`/`Disabled`) → `Ok(false)`.
    fn deps_ready(&self, deps: &[String]) -> Result<bool> {
        Ok(self.unmet_deps(deps)?.is_empty())
    }

    fn wait_settled(&self, name: &str, timeout: Duration) -> ServiceState {
        let deadline = std::time::Instant::now() + timeout;
        let mut map = mutex_lock(&self.runtimes);
        loop {
            let state = map
                .get(name)
                .map(|r| r.state)
                .unwrap_or(ServiceState::Failed);
            if matches!(
                state,
                ServiceState::Running
                    | ServiceState::Succeeded
                    | ServiceState::Failed
                    | ServiceState::Stopped
                    | ServiceState::Disabled
                    | ServiceState::WaitingForDependency
            ) {
                return state;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return state;
            }
            let (guard, result) = self
                .cv
                .wait_timeout(map, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            map = guard;
            if result.timed_out() {
                return map
                    .get(name)
                    .map(|r| r.state)
                    .unwrap_or(ServiceState::Failed);
            }
        }
    }
}

pub struct Supervisor {
    config: Mutex<Config>,
    shared: Arc<Shared>,
    hub: Arc<LogHub>,
    console: Arc<Console>,
    override_path: PathBuf,
    config_path: PathBuf,
    dropins_dir: PathBuf,
    exits: Arc<ExitRegistry>,
    ctl: Mutex<HashMap<String, std::sync::mpsc::Sender<CtlMsg>>>,
    started_at: Instant,
}

enum CtlMsg {
    Start { force: bool },
    Stop,
    Restart,
    Quit,
}

impl Supervisor {
    pub fn new(
        config: Config,
        hub: Arc<LogHub>,
        console: Arc<Console>,
        override_path: PathBuf,
        config_path: PathBuf,
        dropins_dir: PathBuf,
    ) -> Arc<Self> {
        let mut runtimes = HashMap::new();
        for svc in &config.services {
            runtimes.insert(svc.name.clone(), new_runtime(svc.enabled));
        }
        Arc::new(Self {
            config: Mutex::new(config),
            shared: Arc::new(Shared {
                runtimes: Mutex::new(runtimes),
                cv: Condvar::new(),
                stop_all: AtomicBool::new(false),
            }),
            hub,
            console,
            override_path,
            config_path,
            dropins_dir,
            exits: global_exits(),
            ctl: Mutex::new(HashMap::new()),
            started_at: Instant::now(),
        })
    }

    #[must_use]
    pub fn list(&self) -> Vec<ServiceStatus> {
        let map = mutex_lock(&self.shared.runtimes);
        let cfg = mutex_lock(&self.config);
        cfg.services
            .iter()
            .filter_map(|s| {
                map.get(&s.name).map(|rt| ServiceStatus {
                    name: s.name.clone(),
                    state: rt.state,
                    pid: rt.pid,
                    restarts: rt.restarts,
                    liveness_failures: rt.liveness_failures,
                    enabled: rt.enabled,
                    labels: s.labels.clone(),
                })
            })
            .collect()
    }

    pub fn status(&self, name: &str) -> Result<ServiceStatus> {
        let map = mutex_lock(&self.shared.runtimes);
        let cfg = mutex_lock(&self.config);
        let rt = map
            .get(name)
            .ok_or_else(|| Error::UnknownService(name.to_string()))?;
        let labels = cfg
            .services
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.labels.clone())
            .unwrap_or_default();
        Ok(ServiceStatus {
            name: name.to_string(),
            state: rt.state,
            pid: rt.pid,
            restarts: rt.restarts,
            liveness_failures: rt.liveness_failures,
            enabled: rt.enabled,
            labels,
        })
    }

    /// Rich status: direct deps, reverse deps, transitive subgraph, recent events.
    ///
    /// Snapshots under short critical sections (lock order: `runtimes`, then
    /// `config`), then builds the graph and formats event timestamps unlocked.
    ///
    /// When `output` is [`DescribeOutput::Json`], also attaches the raw
    /// source-file service object (`source`).
    pub fn describe(
        &self,
        name: &str,
        output: crate::protocol::DescribeOutput,
    ) -> Result<ServiceDescribe> {
        use crate::protocol::DescribeOutput;
        use crate::service::read_running_identity;

        // --- Snapshot under runtimes (released before config / graph work) ---
        let (mut status, uptime_secs, events, states, pid) = {
            let map = mutex_lock(&self.shared.runtimes);
            let rt = map
                .get(name)
                .ok_or_else(|| Error::UnknownService(name.to_string()))?;

            let status = ServiceStatus {
                name: name.to_string(),
                state: rt.state,
                pid: rt.pid,
                restarts: rt.restarts,
                liveness_failures: rt.liveness_failures,
                enabled: rt.enabled,
                labels: BTreeMap::new(),
            };
            // Uptime only while currently `Running` (`running_since` is cleared otherwise).
            let uptime_secs = if matches!(rt.state, ServiceState::Running) {
                rt.running_since
                    .map(|since| Instant::now().duration_since(since).as_secs())
            } else {
                None
            };

            let pid = rt.pid;

            let start = rt.events.len().saturating_sub(EVENT_RETURN);
            let events: Vec<ServiceEvent> = rt
                .events
                .iter()
                .skip(start)
                .map(RuntimeEvent::to_protocol)
                .collect();

            let states: HashMap<String, ServiceState> =
                map.iter().map(|(n, r)| (n.clone(), r.state)).collect();
            (status, uptime_secs, events, states, pid)
        };

        // Procfs / NSS outside the runtimes lock.
        let running_as = pid.and_then(read_running_identity);

        // --- Snapshot dependency edges under config ---
        let security_context;
        let (depends_on_names, services_deps) = {
            let cfg = mutex_lock(&self.config);
            let svc = cfg
                .services
                .iter()
                .find(|s| s.name == name)
                .ok_or_else(|| Error::UnknownService(name.to_string()))?;
            status.labels = svc.labels.clone();
            security_context = svc.security_context.clone();
            let depends_on_names = svc.depends_on.clone();
            let services_deps: Vec<(String, Vec<String>)> = cfg
                .services
                .iter()
                .map(|s| (s.name.clone(), s.depends_on.clone()))
                .collect();
            (depends_on_names, services_deps)
        };

        // --- Graph construction unlocked ---
        let state_of =
            |n: &str| -> ServiceState { states.get(n).copied().unwrap_or(ServiceState::Pending) };

        let mut depends_on: Vec<DepNode> = depends_on_names
            .iter()
            .map(|d| DepNode {
                name: d.clone(),
                state: state_of(d),
            })
            .collect();
        depends_on.sort_by(|a, b| a.name.cmp(&b.name));

        let mut dependents: Vec<DepNode> = services_deps
            .iter()
            .filter(|(_, deps)| deps.iter().any(|d| d == name))
            .map(|(n, _)| DepNode {
                name: n.clone(),
                state: state_of(n),
            })
            .collect();
        dependents.sort_by(|a, b| a.name.cmp(&b.name));

        // Forward: dep -> service (service depends on dep)
        // Reverse: service -> deps
        let mut forward: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
        for (svc_name, deps) in &services_deps {
            for dep in deps {
                forward
                    .entry(dep.as_str())
                    .or_default()
                    .push(svc_name.as_str());
                reverse
                    .entry(svc_name.as_str())
                    .or_default()
                    .push(dep.as_str());
            }
        }

        let mut visited: HashSet<String> = HashSet::new();
        let mut edge_set: HashSet<(String, String)> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        visited.insert(name.to_string());
        queue.push_back(name.to_string());

        while let Some(cur) = queue.pop_front() {
            if let Some(deps) = reverse.get(cur.as_str()) {
                for &dep in deps {
                    edge_set.insert((dep.to_string(), cur.clone()));
                    if visited.insert(dep.to_string()) {
                        queue.push_back(dep.to_string());
                    }
                }
            }
            if let Some(children) = forward.get(cur.as_str()) {
                for &child in children {
                    edge_set.insert((cur.clone(), child.to_string()));
                    if visited.insert(child.to_string()) {
                        queue.push_back(child.to_string());
                    }
                }
            }
        }

        let mut dep_nodes: Vec<DepNode> = visited
            .iter()
            .map(|n| DepNode {
                name: n.clone(),
                state: state_of(n),
            })
            .collect();
        dep_nodes.sort_by(|a, b| a.name.cmp(&b.name));

        let mut dep_edges: Vec<(String, String)> = edge_set.into_iter().collect();
        dep_edges.sort();

        let source = if matches!(output, DescribeOutput::Json) {
            find_service_source(&self.dropins_dir, &self.config_path, name)?
        } else {
            None
        };

        Ok(ServiceDescribe {
            status,
            uptime_secs,
            depends_on,
            dependents,
            dep_nodes,
            dep_edges,
            events,
            running_as,
            security_context,
            source,
        })
    }

    fn send_ctl(&self, name: &str, msg: CtlMsg) -> Result<()> {
        let ctl = mutex_lock(&self.ctl);
        let tx = ctl
            .get(name)
            .ok_or_else(|| Error::UnknownService(name.to_string()))?;
        tx.send(msg)
            .map_err(|_| Error::Service(name.to_string(), "control channel closed".into()))?;
        Ok(())
    }

    #[must_use = "start may fail if the service is disabled or unknown"]
    pub fn start_service(&self, name: &str, force: bool) -> Result<String> {
        if !self.shared.is_enabled(name)? {
            return Err(Error::Disabled(name.to_string()));
        }
        let cfg = self.service_cfg(name)?;
        let unmet = self.shared.unmet_deps(&cfg.depends_on)?;
        let message = if unmet.is_empty() {
            format!("{name}: starting")
        } else if force {
            format!(
                "{name}: starting with --force (unmet dependencies: {})",
                unmet.join(", ")
            )
        } else {
            format!("{name}: waiting for dependencies ({})", unmet.join(", "))
        };
        self.hub.emit(
            INIT_SERVICE,
            LogLevel::Info,
            format!(
                "request: start {name}{}",
                if force { " --force" } else { "" }
            ),
        );
        self.send_ctl(name, CtlMsg::Start { force })?;
        Ok(message)
    }

    #[must_use = "stop may fail if the service is unknown"]
    pub fn stop_service(&self, name: &str) -> Result<()> {
        self.hub.emit(
            INIT_SERVICE,
            LogLevel::Info,
            format!("request: stop {name}"),
        );
        self.send_ctl(name, CtlMsg::Stop)
    }

    #[must_use = "restart may fail if the service is disabled or unknown"]
    pub fn restart_service(&self, name: &str) -> Result<()> {
        if !self.shared.is_enabled(name)? {
            return Err(Error::Disabled(name.to_string()));
        }
        self.hub.emit(
            INIT_SERVICE,
            LogLevel::Info,
            format!("request: restart {name}"),
        );
        self.send_ctl(name, CtlMsg::Restart)
    }

    #[must_use = "enable/disable may fail on I/O or unknown service"]
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        {
            let mut cfg = mutex_lock(&self.config);
            let svc = cfg
                .get_mut(name)
                .ok_or_else(|| Error::UnknownService(name.to_string()))?;
            svc.enabled = enabled;
        }
        let mut map = crate::config::load_override(&self.override_path)?;
        map.insert(name.to_string(), enabled);
        crate::config::save_override(&self.override_path, &map)?;

        self.hub.emit(
            INIT_SERVICE,
            LogLevel::Info,
            format!(
                "request: {} {name}",
                if enabled { "enable" } else { "disable" }
            ),
        );
        self.shared.set_enabled(name, enabled);
        if enabled {
            self.send_ctl(name, CtlMsg::Start { force: false })?;
        } else {
            self.send_ctl(name, CtlMsg::Stop)?;
        }
        Ok(())
    }

    /// Spawn monitor threads for all services, then run boot sequence.
    #[must_use = "boot reports service start failures"]
    pub fn boot(self: &Arc<Self>) -> Result<()> {
        let services: Vec<ServiceConfig> = mutex_lock(&self.config).services.clone();

        for svc in &services {
            self.spawn_monitor(&svc.name)?;
        }

        ensure_reaper_thread();

        let (foreground, background) = partition_boot(&services)?;

        for name in &background {
            if let Err(e) = self.send_ctl(name, CtlMsg::Start { force: false }) {
                self.hub
                    .emit(INIT_SERVICE, LogLevel::Error, format!("start {name}: {e}"));
            }
        }

        for name in &foreground {
            self.console.starting(name);
            if let Err(e) = self.send_ctl(name, CtlMsg::Start { force: false }) {
                self.hub
                    .emit(INIT_SERVICE, LogLevel::Error, format!("start {name}: {e}"));
                self.console.fail(name);
                continue;
            }
            let state = self.shared.wait_settled(name, BOOT_FG_SETTLE);
            match state {
                ServiceState::Running | ServiceState::Succeeded => self.console.ok(name),
                _ => self.console.fail(name),
            }
        }

        for name in &background {
            let state = self.shared.wait_settled(name, BOOT_BG_SETTLE);
            match state {
                ServiceState::Running
                | ServiceState::Succeeded
                | ServiceState::Starting
                | ServiceState::Pending
                | ServiceState::WaitingForDependency => self.console.ok(name),
                _ => self.console.fail(name),
            }
        }

        Ok(())
    }

    fn spawn_monitor(self: &Arc<Self>, name: &str) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        mutex_lock(&self.ctl).insert(name.to_string(), tx);
        let this = Arc::clone(self);
        let n = name.to_string();
        thread::Builder::new()
            .name(format!("svc-{name}"))
            .spawn(move || this.monitor_loop(n, rx))
            .map_err(|e| Error::Other(e.to_string()))?;
        Ok(())
    }

    fn service_cfg(&self, name: &str) -> Result<ServiceConfig> {
        mutex_lock(&self.config)
            .get(name)
            .cloned()
            .ok_or_else(|| Error::UnknownService(name.to_string()))
    }

    /// Current OpenTelemetry config (for the optional metrics thread).
    ///
    /// Starts from JSON `openTelemetry`, then overlays `$DATA_DIR/etc/otel.env`
    /// and process `OTEL_*` / `ENABLE_TELEMETRY`.
    #[must_use]
    pub fn open_telemetry(&self) -> crate::config::OpenTelemetryConfig {
        let base = mutex_lock(&self.config).open_telemetry.clone();
        crate::otelenv::overlay_config(base)
    }

    /// Daemon snapshot for `Request::Info` / `microinit info`.
    #[must_use]
    pub fn info(&self) -> DaemonInfo {
        let ver = crate::version::info();
        let otel = self.open_telemetry();
        let map = mutex_lock(&self.shared.runtimes);
        let services_total = map.len();
        let services_running = map
            .values()
            .filter(|rt| matches!(rt.state, ServiceState::Running))
            .count();
        let socket = mutex_lock(&self.config).socket.clone();
        DaemonInfo {
            version: ver.version,
            tag_commit: ver.tag_commit,
            build_commit: ver.build_commit,
            build_time: ver.build_time,
            pid: std::process::id(),
            hostname: crate::version::hostname(),
            uptime_secs: self.started_at.elapsed().as_secs(),
            socket,
            services_total,
            services_running,
            otel_enabled: otel.enable,
            otel_endpoint: otel.endpoint,
            otel_protocol: otel.protocol,
            otel_service_name: otel.service_name,
            otel_export_interval_secs: otel.export_interval_secs,
        }
    }

    /// Snapshot metrics for all known services.
    #[must_use]
    pub fn metrics_snapshot(&self) -> Vec<ServiceMetrics> {
        let map = mutex_lock(&self.shared.runtimes);
        let now = Instant::now();
        let mut out: Vec<_> = map
            .iter()
            .map(|(name, rt)| ServiceMetrics {
                name: name.clone(),
                restarts: rt.restarts,
                liveness_failures: rt.liveness_failures,
                pid: rt.pid,
                enabled: rt.enabled,
                state: rt.state,
                uptime_secs: rt
                    .running_since
                    .map(|t| now.saturating_duration_since(t).as_secs_f64())
                    .unwrap_or(0.0),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Apply a newly loaded config: add/remove/restart services as needed.
    pub fn reload(self: &Arc<Self>, new_cfg: Config) -> Result<()> {
        let old = mutex_lock(&self.config).clone();

        if old.socket != new_cfg.socket {
            self.hub.emit(
                INIT_SERVICE,
                LogLevel::Warn,
                "reload: socket change ignored (restart microinit required)",
            );
        }
        if old.logs != new_cfg.logs {
            self.hub.emit(
                INIT_SERVICE,
                LogLevel::Warn,
                "reload: logs.* change ignored (restart microinit required)",
            );
        }
        if old.console != new_cfg.console {
            self.hub.emit(
                INIT_SERVICE,
                LogLevel::Warn,
                "reload: console change ignored (restart microinit required)",
            );
        }

        let old_names: std::collections::HashSet<String> =
            old.services.iter().map(|s| s.name.clone()).collect();
        let new_names: std::collections::HashSet<String> =
            new_cfg.services.iter().map(|s| s.name.clone()).collect();

        // Stop + quit removed services while old definitions are still available.
        for name in old_names.difference(&new_names) {
            self.hub.emit(
                INIT_SERVICE,
                LogLevel::Info,
                format!("reload: removing service {name}"),
            );
            let _ = self.send_ctl(name, CtlMsg::Stop);
            thread::sleep(Duration::from_millis(200));
            if let Some(tx) = mutex_lock(&self.ctl).remove(name) {
                let _ = tx.send(CtlMsg::Quit);
            }
            mutex_lock(&self.shared.runtimes).remove(name);
        }

        let mut applied = new_cfg;
        applied.socket = old.socket.clone();
        applied.logs = old.logs.clone();
        applied.console = old.console.clone();
        *mutex_lock(&self.config) = applied.clone();

        for svc in &applied.services {
            if !old_names.contains(&svc.name) {
                self.hub.emit(
                    INIT_SERVICE,
                    LogLevel::Info,
                    format!("reload: adding service {}", svc.name),
                );
                mutex_lock(&self.shared.runtimes)
                    .insert(svc.name.clone(), new_runtime(svc.enabled));
                self.spawn_monitor(&svc.name)?;
                if svc.enabled {
                    let _ = self.send_ctl(&svc.name, CtlMsg::Start { force: false });
                }
                continue;
            }

            let Some(old_svc) = old.get(&svc.name) else {
                continue;
            };

            if old_svc.enabled != svc.enabled {
                self.shared.set_enabled(&svc.name, svc.enabled);
                if svc.enabled {
                    let _ = self.send_ctl(&svc.name, CtlMsg::Start { force: false });
                } else {
                    let _ = self.send_ctl(&svc.name, CtlMsg::Stop);
                }
            }

            if !definition_eq(old_svc, svc) {
                self.hub.emit(
                    INIT_SERVICE,
                    LogLevel::Info,
                    format!("reload: definition changed for {}", svc.name),
                );
                if svc.enabled {
                    let _ = self.send_ctl(&svc.name, CtlMsg::Restart);
                }
            }
        }

        self.hub
            .emit(INIT_SERVICE, LogLevel::Info, "configuration reloaded");
        Ok(())
    }

    fn monitor_loop(self: Arc<Self>, name: String, rx: std::sync::mpsc::Receiver<CtlMsg>) {
        let mut tracked: Option<i32> = None;
        let mut next_liveness: Option<Instant> = None;

        loop {
            if self.shared.stop_all.load(Ordering::SeqCst) {
                if let Ok(cfg) = self.service_cfg(&name) {
                    self.stop_tracked(&cfg, &mut tracked);
                }
                self.shared.set_state(&name, ServiceState::Stopped, None);
                break;
            }

            let msg = match rx.recv_timeout(CTL_POLL) {
                Ok(m) => Some(m),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };

            if let Some(msg) = msg {
                let cfg = match self.service_cfg(&name) {
                    Ok(c) => c,
                    Err(_) => {
                        // Service removed from config — exit monitor.
                        break;
                    }
                };
                match msg {
                    CtlMsg::Quit => {
                        self.stop_tracked(&cfg, &mut tracked);
                        self.shared.set_state(&name, ServiceState::Stopped, None);
                        break;
                    }
                    CtlMsg::Stop => {
                        self.hub
                            .emit(INIT_SERVICE, LogLevel::Info, format!("stopping {name}"));
                        self.shared.set_state(&name, ServiceState::Stopping, None);
                        self.stop_tracked(&cfg, &mut tracked);
                        next_liveness = None;
                        let enabled = self.shared.is_enabled(&name).unwrap_or(true);
                        if enabled {
                            self.shared.set_state(&name, ServiceState::Stopped, None);
                        } else {
                            self.shared.set_state(&name, ServiceState::Disabled, None);
                        }
                        continue;
                    }
                    CtlMsg::Restart => {
                        self.hub
                            .emit(INIT_SERVICE, LogLevel::Info, format!("restarting {name}"));
                        self.stop_tracked(&cfg, &mut tracked);
                        if let Ok(restart) = cfg.resolve_restart() {
                            let code = run_shell(&restart, &cfg, &HashMap::new()).unwrap_or(1);
                            if cfg.daemon && cfg.is_success(code) {
                                self.shared.set_state(&name, ServiceState::Running, None);
                                next_liveness = Self::schedule_liveness(&cfg);
                                continue;
                            }
                        }
                        if let Err(e) = self.do_start(&cfg, &mut tracked, false) {
                            self.hub
                                .emit(INIT_SERVICE, LogLevel::Error, format!("{name}: {e}"));
                            self.shared.set_state(&name, ServiceState::Failed, None);
                        }
                        next_liveness = Self::schedule_liveness(&cfg);
                        continue;
                    }
                    CtlMsg::Start { force } => {
                        if tracked.is_some() {
                            continue;
                        }
                        if let Err(e) = self.do_start(&cfg, &mut tracked, force) {
                            self.hub
                                .emit(INIT_SERVICE, LogLevel::Error, format!("{name}: {e}"));
                            self.shared.set_state(&name, ServiceState::Failed, None);
                        }
                        next_liveness = Self::schedule_liveness(&cfg);
                        continue;
                    }
                }
            }

            if let Some(pid) = tracked {
                if let Some(code) = self.exits.take(pid) {
                    tracked = None;
                    if let Ok(cfg) = self.service_cfg(&name) {
                        self.on_process_exit(&cfg, &mut tracked, code);
                        next_liveness = Self::schedule_liveness(&cfg);
                    }
                }
            } else if !self.shared.stop_all.load(Ordering::SeqCst) {
                // Start was requested earlier but deps were not ready: retry when they are.
                if matches!(
                    self.shared.current_state(&name),
                    Some(ServiceState::WaitingForDependency)
                ) {
                    if let Ok(cfg) = self.service_cfg(&name) {
                        if let Err(e) = self.do_start(&cfg, &mut tracked, false) {
                            self.hub
                                .emit(INIT_SERVICE, LogLevel::Error, format!("{name}: {e}"));
                            self.shared.set_state(&name, ServiceState::Failed, None);
                        }
                        next_liveness = Self::schedule_liveness(&cfg);
                    }
                }
            }

            if !self.shared.stop_all.load(Ordering::SeqCst) {
                if let Ok(cfg) = self.service_cfg(&name) {
                    self.maybe_liveness(&cfg, &mut tracked, &mut next_liveness);
                }
            }
        }
    }

    fn schedule_liveness(cfg: &ServiceConfig) -> Option<Instant> {
        cfg.liveness_probe
            .as_ref()
            .map(|p| Instant::now() + Duration::from_secs(p.interval))
    }

    /// Periodic health check: on failure, stop and re-run start.
    fn maybe_liveness(
        self: &Arc<Self>,
        cfg: &ServiceConfig,
        tracked: &mut Option<i32>,
        next_liveness: &mut Option<Instant>,
    ) {
        let Some(probe) = cfg.liveness_probe.as_ref() else {
            *next_liveness = None;
            return;
        };

        let state = match self.shared.current_state(&cfg.name) {
            Some(s) => s,
            None => return,
        };
        if !matches!(
            state,
            ServiceState::Running | ServiceState::Succeeded | ServiceState::Failed
        ) {
            return;
        }
        if !self.shared.is_enabled(&cfg.name).unwrap_or(false) {
            return;
        }

        let due = match *next_liveness {
            Some(t) => Instant::now() >= t,
            None => {
                *next_liveness = Some(Instant::now() + Duration::from_secs(probe.interval));
                false
            }
        };
        if !due {
            return;
        }

        let outcome = run_probe(probe, cfg);
        *next_liveness = Some(Instant::now() + Duration::from_secs(probe.interval));

        if outcome.is_ok() {
            if matches!(state, ServiceState::Failed) && !cfg.daemon {
                self.shared
                    .set_state(&cfg.name, ServiceState::Succeeded, None);
            }
            return;
        }

        let reason = match outcome {
            ProbeResult::Fail(r) => r,
            ProbeResult::Ok => unreachable!(),
        };
        self.hub.emit(
            INIT_SERVICE,
            LogLevel::Warn,
            format!("{}: livenessProbe failed ({reason}), restarting", cfg.name),
        );
        self.shared
            .set_state(&cfg.name, ServiceState::Restarting, None);
        self.shared.bump_liveness_failures(&cfg.name, Some(reason));
        self.shared.bump_restarts(&cfg.name);
        self.stop_tracked(cfg, tracked);
        if let Err(e) = self.do_start(cfg, tracked, false) {
            self.hub.emit(
                INIT_SERVICE,
                LogLevel::Error,
                format!("{}: liveness restart: {e}", cfg.name),
            );
            self.shared.set_state(&cfg.name, ServiceState::Failed, None);
        }
        *next_liveness = Self::schedule_liveness(cfg);
    }

    fn stop_tracked(&self, cfg: &ServiceConfig, tracked: &mut Option<i32>) {
        let grace = cfg.shutdown_wait_secs;
        if let Some(pid) = tracked.take() {
            if let Ok(stop) = cfg.resolve_stop() {
                let _ = run_shell(&stop, cfg, &HashMap::new());
            }
            terminate_pid(Pid::from_raw(pid), grace);
            let _ = self
                .exits
                .wait_take(pid, Duration::from_secs(grace.saturating_add(1)));
        } else if let Ok(stop) = cfg.resolve_stop() {
            let _ = run_shell(&stop, cfg, &HashMap::new());
        }
    }

    fn on_process_exit(
        self: &Arc<Self>,
        cfg: &ServiceConfig,
        tracked: &mut Option<i32>,
        code: i32,
    ) {
        let name = &cfg.name;
        self.shared.set_state(name, ServiceState::Pending, None);
        let success = cfg.is_success(code);
        let enabled = self.shared.is_enabled(name).unwrap_or(true);

        if !cfg.daemon {
            let st = if success {
                ServiceState::Succeeded
            } else {
                ServiceState::Failed
            };
            self.shared.set_state(name, st, None);
            return;
        }

        let stop_all = self.shared.stop_all.load(Ordering::SeqCst);
        let should_restart = cfg.restart_policy.should_restart(success) && enabled && !stop_all;
        if !should_restart {
            let st = if success {
                ServiceState::Succeeded
            } else {
                ServiceState::Failed
            };
            self.shared.set_state(name, st, None);
            return;
        }

        self.shared.set_state(name, ServiceState::Restarting, None);
        self.shared.bump_restarts(name);
        self.hub.emit(
            INIT_SERVICE,
            LogLevel::Info,
            format!(
                "{name}: exited {code}, restarting in {}s",
                cfg.restart_backoff
            ),
        );
        thread::sleep(Duration::from_secs(cfg.restart_backoff));
        if let Err(e) = self.do_start(cfg, tracked, false) {
            self.hub
                .emit(INIT_SERVICE, LogLevel::Error, format!("{name}: {e}"));
            self.shared.set_state(name, ServiceState::Failed, None);
        }
    }

    fn do_start(
        self: &Arc<Self>,
        cfg: &ServiceConfig,
        tracked: &mut Option<i32>,
        force: bool,
    ) -> Result<()> {
        let name = &cfg.name;
        if !self.shared.is_enabled(name)? {
            self.shared.set_state(name, ServiceState::Disabled, None);
            return Err(Error::Disabled(name.clone()));
        }

        if !force && !cfg.depends_on.is_empty() && !self.shared.deps_ready(&cfg.depends_on)? {
            if !matches!(
                self.shared.current_state(name),
                Some(ServiceState::WaitingForDependency)
            ) {
                self.hub.emit(
                    INIT_SERVICE,
                    LogLevel::Info,
                    format!(
                        "{name}: waiting for dependencies ({})",
                        cfg.depends_on.join(", ")
                    ),
                );
            }
            self.shared
                .set_state(name, ServiceState::WaitingForDependency, None);
            return Ok(());
        }

        if force && !cfg.depends_on.is_empty() {
            let unmet = self.shared.unmet_deps(&cfg.depends_on)?;
            if !unmet.is_empty() {
                self.hub.emit(
                    INIT_SERVICE,
                    LogLevel::Warn,
                    format!(
                        "{name}: starting with --force (unmet dependencies: {})",
                        unmet.join(", ")
                    ),
                );
            }
        }

        self.shared.set_state(name, ServiceState::Starting, None);
        let cmd = cfg.resolve_start()?;

        let mut c = spawn_shell(&cmd, cfg)?;
        let pid = c.id() as i32;

        if let Some(stdout) = c.stdout.take() {
            capture_stream(self.hub.clone(), name.clone(), LogLevel::Stdout, stdout);
        }
        if let Some(stderr) = c.stderr.take() {
            capture_stream(self.hub.clone(), name.clone(), LogLevel::Stderr, stderr);
        }

        // Transfer wait ownership to the central reaper (init waitpid loop).
        std::mem::forget(c);

        if cfg.daemon {
            // Wait startWaitSecs (or a short SysV daemonize grace when 0) before
            // deciding whether the daemon actually stayed up.
            let wait = if cfg.start_wait_secs > 0 {
                Duration::from_secs(cfg.start_wait_secs)
            } else {
                DAEMONIZE_GRACE
            };
            // Poll so early exits are noticed without busy-spinning for the full wait.
            let deadline = std::time::Instant::now() + wait;
            let mut early_exit = None;
            while std::time::Instant::now() < deadline {
                if let Some(code) = self.exits.take(pid) {
                    early_exit = Some(code);
                    break;
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                thread::sleep(remaining.min(CTL_POLL));
            }
            if early_exit.is_none() {
                early_exit = self.exits.take(pid);
            }

            if let Some(code) = early_exit {
                if cfg.start_wait_secs == 0 && cfg.is_success(code) {
                    // SysV-style script daemonized and exited (only when not
                    // explicitly waiting for the process to stay up).
                    self.shared.set_state(name, ServiceState::Running, None);
                    *tracked = None;
                    return Ok(());
                }
                self.shared.set_state(name, ServiceState::Failed, None);
                return Err(Error::Service(
                    name.clone(),
                    if cfg.start_wait_secs > 0 {
                        format!("exited during startWaitSecs (code {code})")
                    } else {
                        format!("exited with code {code}")
                    },
                ));
            }
            self.shared
                .set_state(name, ServiceState::Running, Some(pid));
            *tracked = Some(pid);
            Ok(())
        } else {
            let Some(code) = self.exits.wait_take(pid, DEP_WAIT) else {
                terminate_pid(Pid::from_raw(pid), STOP_GRACE_SECS);
                let _ = self
                    .exits
                    .wait_take(pid, Duration::from_secs(STOP_GRACE_SECS));
                self.shared.set_state(name, ServiceState::Failed, None);
                return Err(Error::Service(name.clone(), "job timed out".into()));
            };
            *tracked = None;
            if cfg.is_success(code) {
                self.shared.set_state(name, ServiceState::Succeeded, None);
                Ok(())
            } else {
                self.shared.set_state(name, ServiceState::Failed, None);
                Err(Error::Service(
                    name.clone(),
                    format!("exited with code {code}"),
                ))
            }
        }
    }

    pub fn stop_all_ordered(&self) {
        self.shared.stop_all.store(true, Ordering::SeqCst);
        let services = mutex_lock(&self.config).services.clone();
        let order = match shutdown_order(&services) {
            Ok(o) => o,
            Err(e) => {
                self.hub.emit(
                    INIT_SERVICE,
                    LogLevel::Error,
                    format!("shutdown order failed ({e}); using reverse config order"),
                );
                services.iter().rev().map(|s| s.name.clone()).collect()
            }
        };
        for name in &order {
            if let Err(e) = self.send_ctl(name, CtlMsg::Stop) {
                self.hub
                    .emit(INIT_SERVICE, LogLevel::Error, format!("stop {name}: {e}"));
            }
        }
        thread::sleep(SHUTDOWN_STOP_WAIT);
        for name in &order {
            if let Err(e) = self.send_ctl(name, CtlMsg::Quit) {
                self.hub
                    .emit(INIT_SERVICE, LogLevel::Error, format!("quit {name}: {e}"));
            }
        }
        thread::sleep(SHUTDOWN_QUIT_WAIT);
    }
}

/// Compare service definitions ignoring `enabled` (handled separately on reload)
/// and cached `resolved_security` (derived from `securityContext`).
fn definition_eq(a: &ServiceConfig, b: &ServiceConfig) -> bool {
    let mut x = a.clone();
    let mut y = b.clone();
    x.enabled = true;
    y.enabled = true;
    #[cfg(not(target_os = "android"))]
    {
        x.resolved_security = None;
        y.resolved_security = None;
    }
    x == y
}

/// Locate the raw JSON object for `name` in drop-ins (later wins) or main config.
fn find_service_source(
    dropins_dir: &Path,
    config_path: &Path,
    name: &str,
) -> Result<Option<ServiceSource>> {
    // Prefer the last drop-in that defines the service (same merge order as load).
    let mut found: Option<ServiceSource> = None;
    if let Ok(rels) = crate::config::collect_dropin_rel_paths(dropins_dir) {
        for rel in rels {
            let path = dropins_dir.join(&rel);
            if let Some(json) = extract_service_json(&path, name)? {
                found = Some(ServiceSource {
                    path: path.display().to_string(),
                    json,
                });
            }
        }
    }
    if found.is_some() {
        return Ok(found);
    }
    if let Some(json) = extract_service_json(config_path, name)? {
        return Ok(Some(ServiceSource {
            path: config_path.display().to_string(),
            json,
        }));
    }
    Ok(None)
}

fn extract_service_json(path: &Path, name: &str) -> Result<Option<serde_json::Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    let root: serde_json::Value = serde_json::from_str(&data)?;
    let Some(services) = root.get("services").and_then(|v| v.as_array()) else {
        return Ok(None);
    };
    for svc in services {
        if svc.get("name").and_then(|v| v.as_str()) == Some(name) {
            return Ok(Some(svc.clone()));
        }
    }
    Ok(None)
}
