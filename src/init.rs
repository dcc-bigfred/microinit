//! PID 1 / `microinit init` procedure.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use crate::config::{self, Paths, DEFAULT_CONSOLE, DEFAULT_INIT_LOGS_TTY, DEFAULT_LOGS_TTY};
use crate::console::Console;
use crate::constants::{GETTY_RESPAWN_DELAY, INIT_LOOP_SLEEP};
use crate::error::Result;
use crate::ipc::{self, write_frame};
use crate::logs::{boot_note, LogHub};
use crate::protocol::{LogLevel, Request, Response};
use crate::reaper::global_exits;
use crate::shutdown::finalize;
use crate::signals::{self, take_shutdown};
use crate::supervisor::Supervisor;

pub struct InitOpts {
    pub logs_tty: String,
    pub init_logs_tty: String,
    pub console: String,
    pub paths: Paths,
    /// If true, do not run early-boot at all (local / host testing).
    pub skip_early_boot: bool,
    /// If false, continue when early-boot script is missing (default true for PID 1).
    pub require_early_boot: bool,
    /// Force `logs.logToFiles` on (CLI override; config may also enable it).
    pub log_to_files: bool,
}

impl Default for InitOpts {
    fn default() -> Self {
        Self {
            logs_tty: DEFAULT_LOGS_TTY.to_string(),
            init_logs_tty: DEFAULT_INIT_LOGS_TTY.to_string(),
            console: DEFAULT_CONSOLE.to_string(),
            paths: Paths::default(),
            skip_early_boot: false,
            require_early_boot: true,
            log_to_files: false,
        }
    }
}

pub fn run(opts: InitOpts) -> Result<()> {
    // Resolve init-logs path early so pre-hub boot notes reach tty3 as well as stderr.
    let init_logs_preview = opts.init_logs_tty.clone();

    if let Err(e) = signals::install_handlers() {
        boot_note(
            Some(&init_logs_preview),
            &format!("warning: could not install signal handlers: {e}"),
        );
    }

    if opts.skip_early_boot {
        boot_note(
            Some(&init_logs_preview),
            "skipping early-boot (--no-early-boot)",
        );
    } else {
        match crate::early_boot::run(
            &opts.paths,
            &opts.logs_tty,
            &opts.init_logs_tty,
            &opts.console,
        ) {
            Ok(()) => {}
            Err(e) => {
                boot_note(Some(&init_logs_preview), &format!("early-boot failed: {e}"));
                if opts.require_early_boot {
                    return Err(e);
                }
            }
        }
    }

    let mut cfg = config::load_or_create(
        &opts.paths.config,
        &opts.paths.example,
        &opts.paths.override_file,
    )?;

    let logs_tty = if opts.logs_tty != DEFAULT_LOGS_TTY {
        opts.logs_tty.clone()
    } else {
        cfg.logs.tty.clone()
    };
    let init_logs_tty = if opts.init_logs_tty != DEFAULT_INIT_LOGS_TTY {
        opts.init_logs_tty.clone()
    } else {
        cfg.logs.init_tty.clone()
    };
    if opts.log_to_files {
        cfg.logs.log_to_files = true;
    }
    let log_dir = cfg.logs.effective_log_dir();
    let hub = Arc::new(LogHub::new(
        cfg.logs.lines,
        Some(&logs_tty),
        Some(&init_logs_tty),
        log_dir,
    ));
    // Boot phase: init lines → tty3 and stderr (see LogHub::boot_tee_stderr).
    let console = Arc::new(Console::open_with_hub(&opts.console, Some(hub.clone())));

    hub.emit_init(LogLevel::Info, "configuration loaded");
    hub.emit_init(
        LogLevel::Info,
        format!("service logs on {logs_tty}; init logs on {init_logs_tty}"),
    );
    if cfg.logs.log_to_files {
        if let Some(ref dir) = cfg.logs.dir {
            hub.emit_init(LogLevel::Info, format!("logToFiles enabled; writing under {dir}"));
        }
    }

    let socket_path = cfg.socket.clone();
    let lines_default = cfg.logs.lines;
    let override_path = opts.paths.override_file.clone();

    let supervisor = Supervisor::new(cfg, hub.clone(), console.clone(), override_path);

    let sup = Arc::clone(&supervisor);
    let hub_ipc = hub.clone();
    ipc::serve(
        Path::new(&socket_path),
        Arc::new(move |req, stream| handle_ipc(req, stream, &sup, &hub_ipc, lines_default)),
    )?;
    hub.emit_init(LogLevel::Info, format!("IPC listening on {socket_path}"));

    hub.emit_init(LogLevel::Info, "starting services");
    supervisor.boot()?;
    hub.emit_init(LogLevel::Info, "boot complete");

    // After boot / before getty: lifecycle logs only on --init-logs-tty.
    hub.end_boot_tee();
    hub.emit_init(
        LogLevel::Info,
        "boot tee ended; further lifecycle logs only on init-logs-tty",
    );

    // Getty belongs on the real console when we are PID 1; skip for host/local runs.
    if std::process::id() == 1 {
        let console_dev = opts.console.clone();
        thread::spawn(move || getty_respawn(&console_dev));
    } else {
        hub.emit_init(LogLevel::Info, "not PID 1; skipping getty respawn");
    }

    // Exits are published by the process-wide reaper thread started in Supervisor::boot.
    // Opportunistic reap here covers the window before that thread runs and orphans.
    let exits = global_exits();
    loop {
        let _ = signals::sigchld_pending();
        for (pid, code) in signals::reap_zombies() {
            exits.publish(pid.as_raw(), code);
        }

        if let Some(mode) = take_shutdown() {
            hub.emit_init(LogLevel::Info, format!("shutdown requested: {mode}"));
            supervisor.stop_all_ordered();
            let _ = std::fs::remove_file(&socket_path);
            finalize(mode);
        }

        thread::sleep(INIT_LOOP_SLEEP);
    }
}

fn handle_ipc(
    req: Request,
    stream: &mut UnixStream,
    supervisor: &Arc<Supervisor>,
    hub: &Arc<LogHub>,
    lines_default: usize,
) -> Result<()> {
    match req {
        Request::List => {
            write_frame(
                stream,
                &Response::List {
                    services: supervisor.list(),
                },
            )?;
        }
        Request::Status { name } => match supervisor.status(&name) {
            Ok(status) => write_frame(stream, &Response::Status { status })?,
            Err(e) => write_frame(
                stream,
                &Response::Error {
                    message: e.to_string(),
                },
            )?,
        },
        Request::Start { name } => {
            respond_result(stream, supervisor.start_service(&name))?;
        }
        Request::Stop { name } => {
            respond_result(stream, supervisor.stop_service(&name))?;
        }
        Request::Restart { name } => {
            respond_result(stream, supervisor.restart_service(&name))?;
        }
        Request::Enable { name, enabled } => {
            respond_result(stream, supervisor.set_enabled(&name, enabled))?;
        }
        Request::Logs {
            name,
            follow,
            lines,
        } => {
            let n = lines.unwrap_or(lines_default);
            let snapshot = match &name {
                Some(nme) => hub.snapshot_service(nme, n),
                None => hub.snapshot_mixed(n),
            };
            for line in snapshot {
                write_frame(stream, &Response::Log { line })?;
            }
            if follow {
                let rx = hub.subscribe();
                while let Ok(line) = rx.recv() {
                    if let Some(ref nme) = name {
                        if &line.service != nme {
                            continue;
                        }
                    }
                    if write_frame(stream, &Response::Log { line }).is_err() {
                        break;
                    }
                }
            } else {
                write_frame(stream, &Response::Ok)?;
            }
        }
        Request::Shutdown { mode } => {
            write_frame(stream, &Response::Ok)?;
            signals::request_shutdown(mode);
        }
    }
    Ok(())
}

fn respond_result(stream: &mut UnixStream, res: Result<()>) -> Result<()> {
    match res {
        Ok(()) => write_frame(stream, &Response::Ok)?,
        Err(e) => write_frame(
            stream,
            &Response::Error {
                message: e.to_string(),
            },
        )?,
    }
    Ok(())
}

fn getty_respawn(console: &str) {
    let tty = console.trim_start_matches("/dev/");
    loop {
        let status = Command::new("/sbin/getty")
            .args(["-L", "115200", tty, "vt100"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_err() {
            let _ = Command::new("/bin/sh")
                .arg("-l")
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status();
        }
        thread::sleep(GETTY_RESPAWN_DELAY);
    }
}

/// Used when argv0 is `init` or we are PID 1 without a subcommand.
#[must_use]
pub fn should_auto_init() -> bool {
    if std::process::id() == 1 {
        return true;
    }
    std::env::args().next().is_some_and(|arg0| {
        Path::new(&arg0)
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| name == "init")
    })
}
