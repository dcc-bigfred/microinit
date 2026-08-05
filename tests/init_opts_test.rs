//! InitOpts defaults vs supervise wiring (machine_shutdown gate).

use microinit::config::Paths;
use microinit::init::{supervise_opts, InitOpts};

#[test]
fn default_opts_enable_machine_shutdown() {
    let opts = InitOpts::default();
    assert!(opts.machine_shutdown);
    assert!(!opts.skip_early_boot);
    assert!(opts.spawn_getty);
    assert!(opts.attach_ttys);
}

#[test]
fn supervise_opts_disable_machine_shutdown() {
    let opts = supervise_opts(
        "/dev/null".into(),
        Paths::default(),
        "/tmp/test.sock".into(),
        false,
    );
    assert!(
        !opts.machine_shutdown,
        "supervise must not run late unmount or reboot(2)"
    );
    assert!(opts.skip_early_boot);
    assert!(!opts.require_early_boot);
    assert!(!opts.spawn_getty);
    assert!(!opts.attach_ttys);
    assert_eq!(opts.socket, "/tmp/test.sock");
    assert_eq!(opts.console, "/dev/null");
}
