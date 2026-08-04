//! SysV-style `shutdown` CLI for microinit.
//!
//! Asks the running PID 1 / supervise daemon for an ordered poweroff / reboot /
//! halt over `$DATA_DIR/run/microinit.sock` (default `/data/run/microinit.sock`).
//!
//! With the `init` feature (Linux hub builds), falls back to BusyBox
//! `/sbin/{poweroff,reboot,halt}` if the control socket is unavailable.
//! Supervise-only / Android builds have no BusyBox fallback.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[cfg(feature = "init")]
use std::process::Command;

use microinit::cli::{self, ShutdownCliMode};
use microinit::config::{default_socket_path, DEFAULT_SOCKET};
#[cfg(feature = "init")]
use microinit::protocol::ShutdownMode;

fn main() -> ExitCode {
    let mut socket = default_socket_path();
    let mut args: Vec<String> = Vec::new();
    let mut argv = std::env::args().skip(1).peekable();
    while let Some(a) = argv.next() {
        if a == "--socket" {
            match argv.next() {
                Some(p) => socket = PathBuf::from(p),
                None => {
                    eprintln!("shutdown: --socket requires a path");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        if let Some(p) = a.strip_prefix("--socket=") {
            socket = PathBuf::from(p);
            continue;
        }
        args.push(a);
    }

    let mode = match cli::parse_shutdown_args(&args) {
        Ok(ShutdownCliMode::Help) => {
            usage();
            return ExitCode::SUCCESS;
        }
        Ok(ShutdownCliMode::Mode(m)) => m,
        Err(e) => {
            eprintln!("shutdown: {e}");
            usage();
            return ExitCode::from(2);
        }
    };

    if let Err(e) = cli::cmd_shutdown(&socket, mode) {
        #[cfg(feature = "init")]
        {
            let fb = fallback_bin(mode);
            eprintln!("shutdown: microinit: {e} — falling back to {fb}");
            if let Err(e2) = exec_fallback(fb) {
                eprintln!("shutdown: {e2}");
                return ExitCode::FAILURE;
            }
            return ExitCode::SUCCESS;
        }
        #[cfg(not(feature = "init"))]
        {
            eprintln!("shutdown: microinit: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn usage() {
    let name = std::env::args()
        .next()
        .and_then(|p| {
            Path::new(&p)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "shutdown".into());
    eprintln!("usage: {name} [-h|-H|-r|-P] [now]");
    eprintln!("  (default) / -h / -P   power off via microinit");
    eprintln!("  -r                   reboot via microinit");
    eprintln!("  -H                   halt via microinit");
    eprintln!("  --socket PATH        control socket (default {DEFAULT_SOCKET})");
}

#[cfg(feature = "init")]
fn fallback_bin(mode: ShutdownMode) -> &'static str {
    match mode {
        ShutdownMode::Reboot => "/sbin/reboot",
        ShutdownMode::Halt => "/sbin/halt",
        ShutdownMode::Poweroff => "/sbin/poweroff",
        // ShutdownMode is #[non_exhaustive] for downstream crates.
        _ => "/sbin/poweroff",
    }
}

#[cfg(feature = "init")]
fn exec_fallback(bin: &str) -> Result<(), String> {
    let status = Command::new(bin)
        .status()
        .map_err(|e| format!("{bin}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{bin} exited with {status}"))
    }
}
