//! Unit/integration tests for microinit::service

use std::collections::{BTreeMap, HashMap};

use microinit::config::{RestartPolicy, ServiceConfig};
use microinit::service::*;

fn cfg() -> ServiceConfig {
    ServiceConfig {
        name: "t".into(),
        enabled: true,
        daemon: false,
        restart_policy: RestartPolicy::None,
        restart_backoff: 2,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: false,
        depends_on: vec![],
        cmd: None,
        start_cmd: Some("true".into()),
        stop_cmd: Some("true".into()),
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
    }
}

#[test]
fn run_shell_exit_codes() {
    let c = cfg();
    assert_eq!(run_shell("exit 0", &c, &HashMap::new()).unwrap(), 0);
    assert_eq!(run_shell("exit 3", &c, &HashMap::new()).unwrap(), 3);
}

#[test]
fn run_shell_passes_env() {
    let mut c = cfg();
    c.env.insert("FOO".into(), "bar".into());
    let code = run_shell(
        r#"test "$FOO" = bar && test "$EXTRA" = 1"#,
        &c,
        &HashMap::from([("EXTRA".into(), "1".into())]),
    )
    .unwrap();
    assert_eq!(code, 0);
}

#[test]
fn spawn_shell_captures_output() {
    use std::io::Read;
    let c = cfg();
    let mut child = spawn_shell("echo hello-out; echo hello-err >&2", &c).unwrap();
    let mut out = String::new();
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    assert!(out.contains("hello-out"));
}

#[test]
fn terminate_pid_reaps_sleep() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = nix::unistd::Pid::from_raw(child.id() as i32);
    terminate_pid(pid, 1);
    let _ = child.wait();
    // process should be gone
    assert!(matches!(
        nix::sys::signal::kill(pid, None),
        Err(nix::errno::Errno::ESRCH)
    ));
}
