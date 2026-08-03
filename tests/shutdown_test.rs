//! Unit/integration tests for microinit::shutdown

use microinit::protocol::ShutdownMode;
use microinit::signals::{request_shutdown, take_shutdown};

#[test]
fn request_shutdown_modes() {
    let _ = take_shutdown();
    for mode in [
        ShutdownMode::Reboot,
        ShutdownMode::Poweroff,
        ShutdownMode::Halt,
    ] {
        request_shutdown(mode);
        assert_eq!(take_shutdown(), Some(mode));
        assert_eq!(take_shutdown(), None);
    }
}
