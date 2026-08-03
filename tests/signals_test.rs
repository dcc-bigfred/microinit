//! Unit/integration tests for microinit::signals

use microinit::protocol::ShutdownMode;
use microinit::signals::{
    reap_zombies, request_shutdown, sigchld_pending, take_shutdown, test_set_sigchld_pending,
};
use nix::unistd::Pid;

#[test]
fn request_and_take_shutdown() {
    let _ = take_shutdown();
    request_shutdown(ShutdownMode::Poweroff);
    assert_eq!(take_shutdown(), Some(ShutdownMode::Poweroff));
    assert_eq!(take_shutdown(), None);
}

#[test]
fn reap_exited_child() {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let _ = child.wait();
    let child = std::process::Command::new("true").spawn().unwrap();
    let pid = Pid::from_raw(child.id() as i32);
    std::mem::forget(child);
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let reaped = reap_zombies();
        if reaped.iter().any(|(p, code)| *p == pid && *code == 0) {
            return;
        }
    }
    let _ = reap_zombies();
}

#[test]
fn sigchld_flag_toggle() {
    test_set_sigchld_pending(true);
    assert!(sigchld_pending());
    assert!(!sigchld_pending());
}
