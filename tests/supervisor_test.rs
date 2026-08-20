//! Unit/integration tests for microinit::supervisor

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use microinit::config::{Config, LogsConfig, RestartPolicy, ServiceConfig};
use microinit::console::Console;
use microinit::error::Error;
use microinit::logs::LogHub;
use microinit::protocol::ServiceState;
use microinit::supervisor::*;
use microinit::watch::WaitOutcome;
use serial_test::serial;

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);
impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn job(name: &str, start: &str, deps: &[&str], enabled: bool) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        enabled,
        daemon: false,
        restart_policy: RestartPolicy::None,
        restart_backoff: 1,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: false,
        order_priority: 100,
        depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
        cmd: None,
        start_cmd: Some(start.into()),
        stop_cmd: Some("true".into()),
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
        security_context: None,
        #[cfg(not(target_os = "android"))]
        resolved_security: None,
    }
}

fn daemon_cfg(name: &str, start: &str) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        enabled: true,
        daemon: true,
        restart_policy: RestartPolicy::None,
        restart_backoff: 1,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: true,
        order_priority: 100,
        depends_on: vec![],
        cmd: None,
        start_cmd: Some(start.into()),
        stop_cmd: Some("true".into()),
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
        security_context: None,
        #[cfg(not(target_os = "android"))]
        resolved_security: None,
    }
}

fn make_sup(services: Vec<ServiceConfig>) -> (Arc<Supervisor>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "microinit-sup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let override_path = dir.join("override.json");
    let cfg = Config {
        version: 1,
        logs: LogsConfig {
            tty: "/dev/null".into(),
            init_tty: "/dev/null".into(),
            lines: 50,
            dir: None,
            log_to_files: false,
        },
        socket: dir.join("sock").to_string_lossy().into(),
        console: "/dev/null".into(),
        socket_allow_users: Vec::new(),
        open_telemetry: Default::default(),
        services,
    };
    let hub = Arc::new(LogHub::new(50, None, None, None));
    let console = Arc::new(Console::from_writer(Box::new(Sink::default())));
    let config_path = dir.join("microinit.json");
    let dropins = dir.join("dropins");
    let _ = std::fs::create_dir_all(&dropins);
    let sup = Supervisor::new(
        cfg,
        hub,
        console,
        override_path.clone(),
        config_path,
        dropins,
        microinit::protocol::DaemonMode::Supervise,
    );
    (sup, dir)
}

#[test]
#[serial]
fn info_reports_services_and_otel() {
    // Isolate from sibling tests that touch ENABLE_TELEMETRY.
    std::env::remove_var("ENABLE_TELEMETRY");
    std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    std::env::remove_var("OTEL_SDK_DISABLED");

    let (sup, dir) = make_sup(vec![
        job("a", "true", &[], true),
        job("b", "true", &[], true),
    ]);
    let info = sup.info();
    assert_eq!(info.services_total, 2);
    assert_eq!(info.services_running, 0);
    assert!(!info.otel_enabled);
    assert_eq!(info.version, "dev");
    assert!(!info.build_commit.is_empty());
    assert!(info.socket.contains("sock"));
    assert_eq!(info.mode, microinit::protocol::DaemonMode::Supervise);

    std::env::set_var("ENABLE_TELEMETRY", "true");
    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318");
    let info2 = sup.info();
    assert!(info2.otel_enabled);
    assert_eq!(info2.otel_endpoint, "http://127.0.0.1:4318");
    std::env::remove_var("ENABLE_TELEMETRY");
    std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn boot_jobs_succeed_and_fail() {
    let (sup, dir) = make_sup(vec![
        job("ok", "true", &[], true),
        job("bad", "false", &[], true),
    ]);
    sup.boot().unwrap();
    assert_eq!(sup.status("ok").unwrap().state, ServiceState::Succeeded);
    assert_eq!(sup.status("bad").unwrap().state, ServiceState::Failed);
    assert!(sup.list().iter().all(|s| s.enabled));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn watch_subscribe_filters_and_sees_boot() {
    let mut svc = job("ok", "true", &[], true);
    svc.labels.insert("microdns-port".into(), "8080".into());
    let (sup, dir) = make_sup(vec![svc, job("plain", "true", &[], true)]);
    let sub = sup
        .watch_subscribe(vec!["microdns-port".into()])
        .expect("subscribe");
    let WaitOutcome::Snapshot { mut gen, services } = sub.wait_timeout(0, Duration::from_secs(1))
    else {
        panic!("expected seed snapshot");
    };
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].name, "ok");
    let mut last_state = services[0].state;
    sup.boot().unwrap();
    for _ in 0..30 {
        if last_state == ServiceState::Succeeded {
            break;
        }
        match sub.wait_timeout(gen, Duration::from_millis(200)) {
            WaitOutcome::Snapshot { gen: g, services } => {
                gen = g;
                last_state = services[0].state;
            }
            WaitOutcome::Timeout => {}
        }
    }
    assert_eq!(last_state, ServiceState::Succeeded);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn respects_depends_on_order() {
    let marker = std::env::temp_dir().join(format!("microinit-dep-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let first = format!("echo first > {}", marker.to_string_lossy());
    let second = format!("grep -q first {}", marker.to_string_lossy());
    let (sup, dir) = make_sup(vec![
        job("first", &first, &[], true),
        job("second", &second, &["first"], true),
    ]);
    sup.boot().unwrap();
    assert_eq!(sup.status("first").unwrap().state, ServiceState::Succeeded);
    assert_eq!(sup.status("second").unwrap().state, ServiceState::Succeeded);
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn disabled_skipped_at_boot_and_start_refused() {
    let (sup, dir) = make_sup(vec![
        job("on", "true", &[], true),
        job("off", "true", &[], false),
    ]);
    sup.boot().unwrap();
    assert_eq!(sup.status("on").unwrap().state, ServiceState::Succeeded);
    assert_eq!(sup.status("off").unwrap().state, ServiceState::Disabled);
    assert!(matches!(
        sup.start_service("off", false),
        Err(Error::Disabled(_))
    ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn enable_disable_persists_override() {
    let (sup, dir) = make_sup(vec![job("svc", "true", &[], true)]);
    sup.boot().unwrap();
    assert_eq!(sup.status("svc").unwrap().state, ServiceState::Succeeded);

    sup.set_enabled("svc", false).unwrap();
    let override_path = dir.join("override.json");
    let map = microinit::config::load_override(&override_path).unwrap();
    assert_eq!(map.get("svc"), Some(&false));
    thread::sleep(Duration::from_millis(300));
    assert!(!sup.status("svc").unwrap().enabled);

    sup.set_enabled("svc", true).unwrap();
    let map = microinit::config::load_override(&override_path).unwrap();
    assert_eq!(map.get("svc"), Some(&true));
    thread::sleep(Duration::from_millis(400));
    let st = sup.status("svc").unwrap().state;
    assert!(
        matches!(
            st,
            ServiceState::Succeeded | ServiceState::Starting | ServiceState::Running
        ),
        "state={st}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unknown_service_errors() {
    let (sup, dir) = make_sup(vec![job("only", "true", &[], true)]);
    sup.boot().unwrap();
    assert!(matches!(sup.status("nope"), Err(Error::UnknownService(_))));
    assert!(matches!(
        sup.describe("nope", microinit::protocol::DescribeOutput::Human),
        Err(Error::UnknownService(_))
    ));
    assert!(matches!(
        sup.start_service("nope", false),
        Err(Error::UnknownService(_))
    ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn short_lived_daemon_script_marked_running() {
    // SysV-style: start command exits 0 quickly after "daemonizing"
    let (sup, dir) = make_sup(vec![daemon_cfg("fake", "true")]);
    sup.boot().unwrap();
    assert_eq!(sup.status("fake").unwrap().state, ServiceState::Running);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn start_wait_secs_marks_early_crash_failed() {
    let mut cfg = daemon_cfg("crashy", "sleep 0.2");
    cfg.start_wait_secs = 1;
    cfg.background = false;
    let (sup, dir) = make_sup(vec![cfg]);
    sup.boot().unwrap();
    assert_eq!(sup.status("crashy").unwrap().state, ServiceState::Failed);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn start_wait_secs_keeps_stable_daemon_running() {
    let mut cfg = daemon_cfg("stable", "sleep 30");
    cfg.start_wait_secs = 1;
    cfg.background = false;
    let (sup, dir) = make_sup(vec![cfg]);
    sup.boot().unwrap();
    let st = sup.status("stable").unwrap();
    assert_eq!(st.state, ServiceState::Running);
    assert!(st.pid.is_some());
    let _ = sup.stop_service("stable");
    thread::sleep(Duration::from_millis(800));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn shutdown_wait_secs_stops_tracked_daemon() {
    let mut cfg = daemon_cfg("linger", "exec sleep 60");
    cfg.shutdown_wait_secs = 1;
    cfg.background = false;
    let (sup, dir) = make_sup(vec![cfg]);
    sup.boot().unwrap();
    assert_eq!(sup.status("linger").unwrap().state, ServiceState::Running);
    let pid = sup.status("linger").unwrap().pid.expect("pid");
    sup.stop_service("linger").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if matches!(
            sup.status("linger").unwrap().state,
            ServiceState::Stopped | ServiceState::Disabled
        ) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(sup.status("linger").unwrap().state, ServiceState::Stopped);
    assert!(matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn waits_for_dependency_then_starts() {
    let (sup, dir) = make_sup(vec![
        job("dep", "true", &[], false),
        job("child", "true", &["dep"], true),
    ]);
    sup.boot().unwrap();
    thread::sleep(Duration::from_millis(400));
    assert_eq!(
        sup.status("child").unwrap().state,
        ServiceState::WaitingForDependency
    );

    sup.set_enabled("dep", true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if sup.status("child").unwrap().state == ServiceState::Succeeded {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(sup.status("dep").unwrap().state, ServiceState::Succeeded);
    assert_eq!(sup.status("child").unwrap().state, ServiceState::Succeeded);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stop_cancels_waiting_for_dependency() {
    let (sup, dir) = make_sup(vec![
        job("dep", "true", &[], false),
        job("child", "true", &["dep"], true),
    ]);
    sup.boot().unwrap();
    thread::sleep(Duration::from_millis(400));
    assert_eq!(
        sup.status("child").unwrap().state,
        ServiceState::WaitingForDependency
    );

    sup.stop_service("child").unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(sup.status("child").unwrap().state, ServiceState::Stopped);

    // Enabling the dependency must not auto-start a manually stopped child.
    sup.set_enabled("dep", true).unwrap();
    thread::sleep(Duration::from_millis(500));
    assert_eq!(sup.status("child").unwrap().state, ServiceState::Stopped);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn start_force_bypasses_waiting_for_dependency() {
    let (sup, dir) = make_sup(vec![
        job("dep", "true", &[], false),
        job("child", "true", &["dep"], true),
    ]);
    sup.boot().unwrap();
    thread::sleep(Duration::from_millis(400));
    assert_eq!(
        sup.status("child").unwrap().state,
        ServiceState::WaitingForDependency
    );

    let msg = sup.start_service("child", true).unwrap();
    assert!(
        msg.contains("--force") && msg.contains("dep"),
        "unexpected message: {msg}"
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if sup.status("child").unwrap().state == ServiceState::Succeeded {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(sup.status("child").unwrap().state, ServiceState::Succeeded);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn liveness_probe_restarts_oneshot_on_failure() {
    use microinit::config::LivenessProbe;

    let marker = std::env::temp_dir().join(format!(
        "microinit-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&marker);
    let path = marker.to_string_lossy();
    let start = format!("touch {path}");
    let check = format!("test -f {path}");

    let mut svc = job("net", &start, &[], true);
    svc.liveness_probe = Some(LivenessProbe {
        cmd: Some(check),
        http_url: None,
        tcp_addr: None,
        success_exit_codes: vec![0],
        http_accepted_codes: vec![200],
        http_method: "GET".into(),
        interval: 1,
        timeout: 5,
    });

    let (sup, dir) = make_sup(vec![svc]);
    sup.boot().unwrap();
    assert_eq!(sup.status("net").unwrap().state, ServiceState::Succeeded);
    assert!(marker.exists());

    std::fs::remove_file(&marker).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let st = sup.status("net").unwrap();
        if marker.exists() && st.restarts >= 1 && matches!(st.state, ServiceState::Succeeded) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        marker.exists(),
        "start should recreate marker after probe fail"
    );
    assert!(
        sup.status("net").unwrap().restarts >= 1,
        "expected at least one liveness restart"
    );
    assert!(
        sup.status("net").unwrap().liveness_failures >= 1,
        "expected at least one liveness failure counter bump"
    );
    assert_eq!(sup.status("net").unwrap().state, ServiceState::Succeeded);

    let desc = sup
        .describe("net", microinit::protocol::DescribeOutput::Human)
        .unwrap();
    assert!(
        desc.events
            .iter()
            .any(|e| e.kind == microinit::protocol::ServiceEventKind::LivenessFailed),
        "expected liveness_failed event, got {:?}",
        desc.events
    );
    assert!(
        desc.events
            .iter()
            .any(|e| e.kind == microinit::protocol::ServiceEventKind::Restart),
        "expected restart event, got {:?}",
        desc.events
    );
    assert!(
        desc.events.iter().any(|e| {
            e.kind == microinit::protocol::ServiceEventKind::LivenessFailed && e.detail.is_some()
        }),
        "liveness_failed should carry detail"
    );

    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn describe_deps_and_reverse_deps() {
    let (sup, dir) = make_sup(vec![
        job("a", "true", &[], true),
        job("b", "true", &["a"], true),
        job("c", "true", &["b"], true),
    ]);
    sup.boot().unwrap();

    let mid = sup
        .describe("b", microinit::protocol::DescribeOutput::Human)
        .unwrap();
    assert_eq!(mid.depends_on.len(), 1);
    assert_eq!(mid.depends_on[0].name, "a");
    assert_eq!(mid.dependents.len(), 1);
    assert_eq!(mid.dependents[0].name, "c");

    let names: Vec<_> = mid.dep_nodes.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
    assert!(names.contains(&"c"));
    assert!(mid.dep_edges.contains(&("a".into(), "b".into())));
    assert!(mid.dep_edges.contains(&("b".into(), "c".into())));

    let leaf = sup
        .describe("c", microinit::protocol::DescribeOutput::Human)
        .unwrap();
    assert_eq!(leaf.depends_on[0].name, "b");
    assert!(leaf.dependents.is_empty());
    assert!(leaf.dep_edges.contains(&("a".into(), "b".into())));
    assert!(leaf.dep_edges.contains(&("b".into(), "c".into())));

    // Lifecycle events recorded during boot.
    assert!(
        mid.events.iter().any(|e| {
            e.kind == microinit::protocol::ServiceEventKind::StateChange
                && e.to == Some(ServiceState::Succeeded)
        }),
        "expected state_change to succeeded, got {:?}",
        mid.events
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn describe_enable_disable_records_state_change() {
    let (sup, dir) = make_sup(vec![job("svc", "true", &[], true)]);
    sup.boot().unwrap();
    sup.set_enabled("svc", false).unwrap();
    thread::sleep(Duration::from_millis(200));

    let desc = sup
        .describe("svc", microinit::protocol::DescribeOutput::Human)
        .unwrap();
    assert!(
        desc.events.iter().any(|e| {
            e.kind == microinit::protocol::ServiceEventKind::StateChange
                && e.to == Some(ServiceState::Disabled)
        }),
        "expected disable state_change, got {:?}",
        desc.events
    );
    assert!(desc.events.len() <= microinit::constants::EVENT_RETURN);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn describe_event_ring_returns_at_most_event_return() {
    let (sup, dir) = make_sup(vec![job("svc", "true", &[], true)]);
    sup.boot().unwrap();
    // Generate more transitions than the ring can hold.
    let n = microinit::constants::EVENT_RING_CAP
        .saturating_mul(3)
        .max(8);
    for i in 0..n {
        sup.set_enabled("svc", i % 2 == 0).unwrap();
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(100));

    let desc = sup
        .describe("svc", microinit::protocol::DescribeOutput::Human)
        .unwrap();
    assert_eq!(
        desc.events.len(),
        microinit::constants::EVENT_RETURN,
        "describe must return exactly EVENT_RETURN events when the ring is full, got {}",
        desc.events.len()
    );
    // Oldest → newest: last event should be a state_change from the toggles.
    let last = desc.events.last().expect("non-empty");
    assert_eq!(
        last.kind,
        microinit::protocol::ServiceEventKind::StateChange
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn describe_deps_lists_are_sorted() {
    let (sup, dir) = make_sup(vec![
        job("z", "true", &[], true),
        job("m", "true", &["z", "a"], true),
        job("a", "true", &[], true),
        job("y", "true", &["m"], true),
        job("b", "true", &["m"], true),
    ]);
    sup.boot().unwrap();

    let mid = sup
        .describe("m", microinit::protocol::DescribeOutput::Human)
        .unwrap();
    let dep_names: Vec<_> = mid.depends_on.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(dep_names, vec!["a", "z"]);
    let req_names: Vec<_> = mid.dependents.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(req_names, vec!["b", "y"]);

    let _ = std::fs::remove_dir_all(dir);
}
