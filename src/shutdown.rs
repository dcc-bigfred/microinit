//! Ordered shutdown: stop services, sync, reboot/poweroff/halt.

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

    invoke_reboot(mode);

    // reboot(2) returned — not actually able to reboot. Exit rather than hang.
    eprintln!("microinit: reboot({mode}) failed; exiting");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn invoke_reboot(mode: ShutdownMode) {
    use nix::sys::reboot::{reboot, RebootMode};

    #[allow(unreachable_patterns)] // ShutdownMode is #[non_exhaustive]
    let rb = match mode {
        ShutdownMode::Reboot => RebootMode::RB_AUTOBOOT,
        ShutdownMode::Poweroff => RebootMode::RB_POWER_OFF,
        ShutdownMode::Halt => RebootMode::RB_HALT_SYSTEM,
        _ => RebootMode::RB_POWER_OFF,
    };
    let _ = reboot(rb);
}

/// Android Bionic does not expose `reboot(3)` via the `libc`/`nix` crates the
/// same way glibc/musl do, but the kernel syscall is identical.
#[cfg(target_os = "android")]
fn invoke_reboot(mode: ShutdownMode) {
    #[allow(unreachable_patterns)] // ShutdownMode is #[non_exhaustive]
    let cmd = match mode {
        ShutdownMode::Reboot => libc::LINUX_REBOOT_CMD_RESTART,
        ShutdownMode::Poweroff => libc::LINUX_REBOOT_CMD_POWER_OFF,
        ShutdownMode::Halt => libc::LINUX_REBOOT_CMD_HALT,
        _ => libc::LINUX_REBOOT_CMD_POWER_OFF,
    };
    // SAFETY: reboot(2) with the documented magic + command; returns only on failure.
    let _ = unsafe {
        libc::syscall(
            libc::SYS_reboot,
            libc::LINUX_REBOOT_MAGIC1 as libc::c_long,
            libc::LINUX_REBOOT_MAGIC2 as libc::c_long,
            cmd as libc::c_long,
            std::ptr::null::<libc::c_void>(),
        )
    };
}
