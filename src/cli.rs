//! CLI handlers that talk to the running `microinit init` daemon via IPC.

use std::io::{self, Write};
use std::path::Path;

use crate::error::{Error, Result};
use crate::ipc::{read_frame, request, write_frame};
use crate::protocol::{Request, Response, ShutdownMode};

/// Result of parsing SysV-style `shutdown` argv (excluding `--socket`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownCliMode {
    Help,
    Mode(ShutdownMode),
}

/// Parse SysV-compatible shutdown flags (`-h`/`-P`/`-r`/`-H`, `now`, …).
pub fn parse_shutdown_args(
    args: &[impl AsRef<str>],
) -> std::result::Result<ShutdownCliMode, String> {
    let mut mode = ShutdownMode::Poweroff;
    let mut seen = false;

    for a in args {
        let a = a.as_ref();
        match a {
            "--help" | "help" => return Ok(ShutdownCliMode::Help),
            "-h" | "-P" | "--poweroff" | "poweroff" => {
                if seen && mode != ShutdownMode::Poweroff {
                    return Err("conflicting mode flags".into());
                }
                mode = ShutdownMode::Poweroff;
                seen = true;
            }
            "-r" | "--reboot" | "reboot" => {
                if seen && mode != ShutdownMode::Reboot {
                    return Err("conflicting mode flags".into());
                }
                mode = ShutdownMode::Reboot;
                seen = true;
            }
            "-H" | "--halt" | "halt" => {
                if seen && mode != ShutdownMode::Halt {
                    return Err("conflicting mode flags".into());
                }
                mode = ShutdownMode::Halt;
                seen = true;
            }
            "now" | "+0" | "0" => {
                // SysV compatibility; delayed shutdown is not implemented.
            }
            _ if a.starts_with('-') => {
                return Err(format!("unknown option {a:?}"));
            }
            _ => {
                // Ignore wall message / unsupported time specs.
            }
        }
    }
    Ok(ShutdownCliMode::Mode(mode))
}

/// Ask the daemon to begin ordered shutdown (`poweroff` / `reboot` / `halt`).
pub fn cmd_shutdown(socket: &Path, mode: ShutdownMode) -> Result<()> {
    simple_ok(socket, Request::Shutdown { mode })
}

pub fn cmd_list(socket: &Path) -> Result<()> {
    match request(socket, &Request::List)? {
        Response::List { services } => {
            println!(
                "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10}",
                "NAME", "STATE", "PID", "RESTARTS", "ENABLED", "LIVE_FAIL"
            );
            for s in services {
                let pid = s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
                println!(
                    "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10}",
                    s.name,
                    s.state.to_string(),
                    pid,
                    s.restarts,
                    if s.enabled { "yes" } else { "no" },
                    s.liveness_failures
                );
            }
            Ok(())
        }
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

pub fn cmd_start(socket: &Path, name: &str, force: bool) -> Result<()> {
    match request(
        socket,
        &Request::Start {
            name: name.into(),
            force,
        },
    )? {
        Response::Ok { message } => {
            if let Some(msg) = message {
                println!("{msg}");
            }
            Ok(())
        }
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

pub fn cmd_stop(socket: &Path, name: &str) -> Result<()> {
    simple_ok(socket, Request::Stop { name: name.into() })
}

pub fn cmd_restart(socket: &Path, name: &str) -> Result<()> {
    simple_ok(socket, Request::Restart { name: name.into() })
}

pub fn cmd_enable(socket: &Path, name: &str) -> Result<()> {
    simple_ok(
        socket,
        Request::Enable {
            name: name.into(),
            enabled: true,
        },
    )
}

pub fn cmd_disable(socket: &Path, name: &str) -> Result<()> {
    simple_ok(
        socket,
        Request::Enable {
            name: name.into(),
            enabled: false,
        },
    )
}

pub fn cmd_logs(
    socket: &Path,
    name: Option<String>,
    follow: bool,
    lines: Option<usize>,
) -> Result<()> {
    let mut stream = crate::ipc::connect(socket)?;
    write_frame(
        &mut stream,
        &Request::Logs {
            name,
            follow,
            lines,
        },
    )?;

    let stdout = io::stdout();
    let mut out = stdout.lock();
    loop {
        let resp: Response = read_frame(&mut stream)?;
        match resp {
            Response::Log { line } => {
                writeln!(out, "[{}] {}: {}", line.ts, line.service, line.msg)?;
                out.flush()?;
            }
            Response::Ok { .. } => break,
            Response::Error { message } => return Err(Error::Ipc(message)),
            other => return Err(Error::Ipc(format!("unexpected: {other:?}"))),
        }
    }
    Ok(())
}

fn simple_ok(socket: &Path, req: Request) -> Result<()> {
    match request(socket, &req)? {
        Response::Ok { message } => {
            if let Some(msg) = message {
                println!("{msg}");
            }
            Ok(())
        }
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}
