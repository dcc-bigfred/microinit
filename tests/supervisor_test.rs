//! Unit/integration tests for microinit::supervisor

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use microinit::config::{Config, LogsConfig, ServiceConfig};
use microinit::console::Console;
use microinit::error::Error;
use microinit::logs::LogHub;
use microinit::protocol::ServiceState;
use microinit::supervisor::*;

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
        restart: false,
        restart_backoff: 1,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: false,
        depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
        cmd: None,
        start_cmd: Some(start.into()),
        stop_cmd: Some("true".into()),
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
    }
}

fn daemon_cfg(name: &str, start: &str) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        enabled: true,
        daemon: true,
        restart: false,
        restart_backoff: 1,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: true,
        depends_on: vec![],
        cmd: None,
        start_cmd: Some(start.into()),
        stop_cmd: Some("true".into()),
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
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
        open_telemetry: Default::default(),
        services,
    };
    let hub = Arc::new(LogHub::new(50, None, None, None));
    let console = Arc::new(Console::from_writer(Box::new(Sink::default())));
    let sup = Supervisor::new(cfg, hub, console, override_path.clone());
    (sup, dir)
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
    assert!(matches!(sup.start_service("off"), Err(Error::Disabled(_))));
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
        sup.start_service("nope"),
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
