//! Signal handling for PID 1: SIGCHLD reap, shutdown signals.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;

use crate::protocol::ShutdownMode;

const SHUT_NONE: u8 = 0;
const SHUT_REBOOT: u8 = 1;
const SHUT_POWEROFF: u8 = 2;
const SHUT_HALT: u8 = 3;

static SHUTDOWN_FLAG: AtomicU8 = AtomicU8::new(SHUT_NONE);
static GOT_SIGCHLD: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigchld(_: libc::c_int) {
    // Async-signal-safe: only an atomic store.
    GOT_SIGCHLD.store(true, Ordering::SeqCst);
}

extern "C" fn handle_reboot(_: libc::c_int) {
    SHUTDOWN_FLAG.store(SHUT_REBOOT, Ordering::SeqCst);
}

extern "C" fn handle_halt(_: libc::c_int) {
    SHUTDOWN_FLAG.store(SHUT_HALT, Ordering::SeqCst);
}

extern "C" fn handle_poweroff(_: libc::c_int) {
    SHUTDOWN_FLAG.store(SHUT_POWEROFF, Ordering::SeqCst);
}

/// Install PID 1 signal handlers.
///
/// # Safety
///
/// Handlers only perform atomic stores (async-signal-safe). `sigaction` replaces
/// process-wide dispositions; call once during init before spawning service threads.
pub fn install_handlers() -> nix::Result<()> {
    let sa_chld = SigAction::new(
        SigHandler::Handler(handle_sigchld),
        SaFlags::SA_RESTART | SaFlags::SA_NOCLDSTOP,
        SigSet::empty(),
    );
    // SAFETY: handlers are async-signal-safe (atomic stores only). We intentionally
    // replace previous dispositions for these signals on the init process.
    unsafe {
        sigaction(Signal::SIGCHLD, &sa_chld)?;
        sigaction(
            Signal::SIGTERM,
            &SigAction::new(
                SigHandler::Handler(handle_reboot),
                SaFlags::SA_RESTART,
                SigSet::empty(),
            ),
        )?;
        sigaction(
            Signal::SIGINT,
            &SigAction::new(
                SigHandler::Handler(handle_reboot),
                SaFlags::SA_RESTART,
                SigSet::empty(),
            ),
        )?;
        sigaction(
            Signal::SIGUSR1,
            &SigAction::new(
                SigHandler::Handler(handle_halt),
                SaFlags::SA_RESTART,
                SigSet::empty(),
            ),
        )?;
        sigaction(
            Signal::SIGUSR2,
            &SigAction::new(
                SigHandler::Handler(handle_poweroff),
                SaFlags::SA_RESTART,
                SigSet::empty(),
            ),
        )?;
    }
    Ok(())
}

fn mode_to_flag(mode: ShutdownMode) -> u8 {
    match mode {
        ShutdownMode::Reboot => SHUT_REBOOT,
        ShutdownMode::Poweroff => SHUT_POWEROFF,
        ShutdownMode::Halt => SHUT_HALT,
    }
}

fn flag_to_mode(flag: u8) -> Option<ShutdownMode> {
    match flag {
        SHUT_REBOOT => Some(ShutdownMode::Reboot),
        SHUT_POWEROFF => Some(ShutdownMode::Poweroff),
        SHUT_HALT => Some(ShutdownMode::Halt),
        _ => None,
    }
}

/// Take a pending shutdown request, if any.
#[must_use]
pub fn take_shutdown() -> Option<ShutdownMode> {
    flag_to_mode(SHUTDOWN_FLAG.swap(SHUT_NONE, Ordering::SeqCst))
}

pub fn request_shutdown(mode: ShutdownMode) {
    SHUTDOWN_FLAG.store(mode_to_flag(mode), Ordering::SeqCst);
}

#[must_use]
pub fn sigchld_pending() -> bool {
    GOT_SIGCHLD.swap(false, Ordering::SeqCst)
}

/// Test helper: set the SIGCHLD pending flag.
#[doc(hidden)]
pub fn test_set_sigchld_pending(v: bool) {
    GOT_SIGCHLD.store(v, Ordering::SeqCst);
}

/// Encode shell-style exit status for a fatal signal (128 + signo).
fn signal_exit_code(sig: nix::sys::signal::Signal) -> i32 {
    128i32.saturating_add(sig as i32)
}

/// Reap all exited children (WNOHANG). Returns (pid, exit_code) pairs.
///
/// `exit_code` encodes signal death as `128 + signo` (shell convention).
///
/// # Allocation
///
/// Allocates a `Vec` for the batch of exits observed in this call (bounded by
/// the number of ready zombies; typically small).
pub fn reap_zombies() -> Vec<(Pid, i32)> {
    let mut out = Vec::new();
    loop {
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, code)) => {
                debug_assert!((0..=255).contains(&code));
                out.push((pid, code));
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => out.push((pid, signal_exit_code(sig))),
            Ok(WaitStatus::StillAlive) | Err(nix::errno::Errno::ECHILD) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    out
}
