//! Service supervisor: one monitor thread per service, restart with backoff.
//!
//! Child process waits are owned by the central PID 1 reaper ([`ExitRegistry`]),
//! not by `std::process::Child::wait`, to avoid racing `waitpid(-1)`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nix::unistd::Pid;

use crate::config::{Config, ServiceConfig};
use crate::console::Console;
use crate::constants::{
    BOOT_BG_SETTLE, BOOT_FG_SETTLE, CTL_POLL, DAEMONIZE_GRACE, DEP_WAIT, SHUTDOWN_QUIT_WAIT,
    SHUTDOWN_STOP_WAIT, STOP_GRACE_SECS,
};
use crate::error::{Error, Result};
use crate::graph::{partition_boot, shutdown_order};
use crate::logs::{capture_stream, LogHub, INIT_SERVICE};
use crate::protocol::{LogLevel, ServiceState, ServiceStatus};
use crate::reaper::{ensure_reaper_thread, global_exits, ExitRegistry};
use crate::service::{run_shell, spawn_shell, terminate_pid};
use crate::syncutil::mutex_lock;

#[derive(Debug)]
struct Runtime {
    state: ServiceState,
    pid: Option<i32>,
    restarts: u32,
    enabled: bool,
    running_since: Option<Instant>,
}

/// Snapshot of a service for metrics exporters.
#[derive(Debug, Clone)]
pub struct ServiceMetrics {
    pub name: String,
    pub restarts: u32,
    pub pid: Option<i32>,
    pub enabled: bool,
    pub state: ServiceState,
    pub uptime_secs: f64,
}

struct Shared {
    runtimes: Mutex<HashMap<String, Runtime>>,
    cv: Condvar,
    stop_all: AtomicBool,
}

impl Shared {
    fn set_state(&self, name: &str, state: ServiceState, pid: Option<i32>) {
        let mut map = mutex_lock(&self.runtimes);
        if let Some(rt) = map.get_mut(name) {
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
            match state {
                ServiceState::Running => {
                    if rt.running_since.is_none() {
                        rt.running_since = Some(Instant::now());
                    }
                }
                ServiceState::Starting
                    | ServiceState::Restarting
                    | ServiceState::WaitingForDependency => {}
                _ => rt.running_since = None,
            }
        }
        self.cv.notify_all();
    }

    fn bump_restarts(&self, name: &str) {
        if let Some(rt) = mutex_lock(&self.runtimes).get_mut(name) {
            rt.restarts = rt.restarts.saturating_add(1);
        }
    }

    fn set_enabled(&self, name: &str, enabled: bool) {
        if let Some(rt) = mutex_lock(&self.runtimes).get_mut(name) {
            rt.enabled = enabled;
            if !enabled {
                rt.state = ServiceState::Disabled;
                rt.pid = None;
            } else if matches!(rt.state, ServiceState::Disabled) {
                rt.state = ServiceState::Pending;
            }
        }
        self.cv.notify_all();
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

    /// `Ok(true)` when every dependency is `Running` or `Succeeded`.
    /// Missing deps → error. Not-yet-ready (including `Failed`/`Disabled`) → `Ok(false)`.
    fn deps_ready(&self, deps: &[String]) -> Result<bool> {
        let map = mutex_lock(&self.runtimes);
        for dep in deps {
            let Some(rt) = map.get(dep) else {
                return Err(Error::UnknownService(dep.clone()));
            };
            match rt.state {
                ServiceState::Running | ServiceState::Succeeded => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
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
    exits: Arc<ExitRegistry>,
    ctl: Mutex<HashMap<String, std::sync::mpsc::Sender<CtlMsg>>>,
}

enum CtlMsg {
    Start,
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
    ) -> Arc<Self> {
        let mut runtimes = HashMap::new();
        for svc in &config.services {
            runtimes.insert(
                svc.name.clone(),
                Runtime {
                    state: if svc.enabled {
                        ServiceState::Pending
                    } else {
                        ServiceState::Disabled
                    },
                    pid: None,
                    restarts: 0,
                    enabled: svc.enabled,
                    running_since: None,
                },
            );
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
            exits: global_exits(),
            ctl: Mutex::new(HashMap::new()),
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
                    enabled: rt.enabled,
                })
            })
            .collect()
    }

    pub fn status(&self, name: &str) -> Result<ServiceStatus> {
        let map = mutex_lock(&self.shared.runtimes);
        let rt = map
            .get(name)
            .ok_or_else(|| Error::UnknownService(name.to_string()))?;
        Ok(ServiceStatus {
            name: name.to_string(),
            state: rt.state,
            pid: rt.pid,
            restarts: rt.restarts,
            enabled: rt.enabled,
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
    pub fn start_service(&self, name: &str) -> Result<()> {
        if !self.shared.is_enabled(name)? {
            return Err(Error::Disabled(name.to_string()));
        }
        self.hub.emit(
            INIT_SERVICE,
            LogLevel::Info,
            format!("request: start {name}"),
        );
        self.send_ctl(name, CtlMsg::Start)
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
            self.send_ctl(name, CtlMsg::Start)?;
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
            if let Err(e) = self.send_ctl(name, CtlMsg::Start) {
                self.hub
                    .emit(INIT_SERVICE, LogLevel::Error, format!("start {name}: {e}"));
            }
        }

        for name in &foreground {
            self.console.starting(name);
            if let Err(e) = self.send_ctl(name, CtlMsg::Start) {
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
    #[must_use]
    pub fn open_telemetry(&self) -> crate::config::OpenTelemetryConfig {
        mutex_lock(&self.config).open_telemetry.clone()
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
                mutex_lock(&self.shared.runtimes).insert(
                    svc.name.clone(),
                    Runtime {
                        state: if svc.enabled {
                            ServiceState::Pending
                        } else {
                            ServiceState::Disabled
                        },
                        pid: None,
                        restarts: 0,
                        enabled: svc.enabled,
                        running_since: None,
                    },
                );
                self.spawn_monitor(&svc.name)?;
                if svc.enabled {
                    let _ = self.send_ctl(&svc.name, CtlMsg::Start);
                }
                continue;
            }

            let Some(old_svc) = old.get(&svc.name) else {
                continue;
            };

            if old_svc.enabled != svc.enabled {
                self.shared.set_enabled(&svc.name, svc.enabled);
                if svc.enabled {
                    let _ = self.send_ctl(&svc.name, CtlMsg::Start);
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
                                continue;
                            }
                        }
                        if let Err(e) = self.do_start(&cfg, &mut tracked) {
                            self.hub
                                .emit(INIT_SERVICE, LogLevel::Error, format!("{name}: {e}"));
                            self.shared.set_state(&name, ServiceState::Failed, None);
                        }
                        continue;
                    }
                    CtlMsg::Start => {
                        if tracked.is_some() {
                            continue;
                        }
                        if let Err(e) = self.do_start(&cfg, &mut tracked) {
                            self.hub
                                .emit(INIT_SERVICE, LogLevel::Error, format!("{name}: {e}"));
                            self.shared.set_state(&name, ServiceState::Failed, None);
                        }
                        continue;
                    }
                }
            }

            if let Some(pid) = tracked {
                if let Some(code) = self.exits.take(pid) {
                    tracked = None;
                    if let Ok(cfg) = self.service_cfg(&name) {
                        self.on_process_exit(&cfg, &mut tracked, code);
                    }
                }
            } else if !self.shared.stop_all.load(Ordering::SeqCst) {
                // Start was requested earlier but deps were not ready: retry when they are.
                if matches!(
                    self.shared.current_state(&name),
                    Some(ServiceState::WaitingForDependency)
                ) {
                    if let Ok(cfg) = self.service_cfg(&name) {
                        if let Err(e) = self.do_start(&cfg, &mut tracked) {
                            self.hub
                                .emit(INIT_SERVICE, LogLevel::Error, format!("{name}: {e}"));
                            self.shared.set_state(&name, ServiceState::Failed, None);
                        }
                    }
                }
            }
        }
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

        if success {
            self.shared.set_state(name, ServiceState::Succeeded, None);
            return;
        }

        if cfg.restart && enabled && !self.shared.stop_all.load(Ordering::SeqCst) {
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
            if let Err(e) = self.do_start(cfg, tracked) {
                self.hub
                    .emit(INIT_SERVICE, LogLevel::Error, format!("{name}: {e}"));
                self.shared.set_state(name, ServiceState::Failed, None);
            }
        } else {
            self.shared.set_state(name, ServiceState::Failed, None);
        }
    }

    fn do_start(self: &Arc<Self>, cfg: &ServiceConfig, tracked: &mut Option<i32>) -> Result<()> {
        let name = &cfg.name;
        if !self.shared.is_enabled(name)? {
            self.shared.set_state(name, ServiceState::Disabled, None);
            return Err(Error::Disabled(name.clone()));
        }

        if !cfg.depends_on.is_empty() && !self.shared.deps_ready(&cfg.depends_on)? {
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

/// Compare service definitions ignoring `enabled` (handled separately on reload).
fn definition_eq(a: &ServiceConfig, b: &ServiceConfig) -> bool {
    let mut x = a.clone();
    let mut y = b.clone();
    x.enabled = true;
    y.enabled = true;
    x == y
}
