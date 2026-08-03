//! Ordered shutdown: stop services, sync, reboot/poweroff/halt.

use nix::sys::reboot::{reboot, RebootMode};
use nix::unistd::sync;

use crate::protocol::ShutdownMode;

/// Final reboot syscall. Call after services are stopped and filesystems synced.
///
/// When not PID 1 (local/host testing), or if `reboot(2)` fails, exits the process
/// instead of spinning forever — spin is only a last resort if `exit` itself fails.
pub fn finalize(mode: ShutdownMode) -> ! {
    sync();

    if std::process::id() != 1 {
        // Host testing: never call reboot(2); clean process exit.
        std::process::exit(0);
    }

    let rb = match mode {
        ShutdownMode::Reboot => RebootMode::RB_AUTOBOOT,
        ShutdownMode::Poweroff => RebootMode::RB_POWER_OFF,
        ShutdownMode::Halt => RebootMode::RB_HALT_SYSTEM,
    };
    let _ = reboot(rb);

    // reboot(2) returned — not actually able to reboot. Exit rather than hang.
    eprintln!("microinit: reboot({mode}) failed; exiting");
    std::process::exit(1);
}
