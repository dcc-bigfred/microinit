//! CLI handlers that talk to the running `microinit init` daemon via IPC.

use std::io::{self, Write};
use std::path::Path;

use crate::error::{Error, Result};
use crate::ipc::{read_frame, request, write_frame};
use crate::protocol::{Request, Response};

pub fn cmd_list(socket: &Path) -> Result<()> {
    match request(socket, &Request::List)? {
        Response::List { services } => {
            println!(
                "{:<20} {:<12} {:>8} {:>8} {:>8}",
                "NAME", "STATE", "PID", "RESTARTS", "ENABLED"
            );
            for s in services {
                let pid = s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
                println!(
                    "{:<20} {:<12} {:>8} {:>8} {:>8}",
                    s.name,
                    s.state.to_string(),
                    pid,
                    s.restarts,
                    if s.enabled { "yes" } else { "no" }
                );
            }
            Ok(())
        }
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

pub fn cmd_start(socket: &Path, name: &str) -> Result<()> {
    simple_ok(socket, Request::Start { name: name.into() })
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
            Response::Ok => break,
            Response::Error { message } => return Err(Error::Ipc(message)),
            other => return Err(Error::Ipc(format!("unexpected: {other:?}"))),
        }
    }
    Ok(())
}

fn simple_ok(socket: &Path, req: Request) -> Result<()> {
    match request(socket, &req)? {
        Response::Ok => Ok(()),
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}
