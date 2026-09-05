//! PID 1 / `microinit init` / `microinit supervise` procedure.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

#[cfg(feature = "init")]
use std::process::{Command, Stdio};

use crate::config::{self, Paths, DEFAULT_CONSOLE, DEFAULT_INIT_LOGS_TTY, DEFAULT_LOGS_TTY};
use crate::console::Console;
#[cfg(feature = "init")]
use crate::constants::GETTY_RESPAWN_DELAY;
use crate::constants::{INIT_LOOP_SLEEP, MAX_WATCH_FOLLOWERS, WATCH_HEARTBEAT};
use crate::error::Result;
use crate::ipc::{self, write_frame};
use crate::logs::{boot_note, LogHub};
use crate::protocol::{LogLevel, Request, Response};
use crate::reaper::global_exits;
use crate::signals::{self, take_shutdown};
use crate::supervisor::Supervisor;
use crate::watch::WaitOutcome;

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
    /// Fallback path for early-boot capture when the config cannot be loaded
    /// (script failed before mounting the data root). Best-effort.
    pub early_boot_logs_path: Option<PathBuf>,
    /// Spawn getty on the console when PID 1 (full init only).
    pub spawn_getty: bool,
    /// Attach service/init TTYs to LogHub (false for container supervise).
    pub attach_ttys: bool,
    /// Control socket path (overrides JSON after load).
    pub socket: String,
    /// If true (`init`), run late unmount then reboot/poweroff/halt.
    /// If false (`supervise`), only stop services, sync, and exit.
    pub machine_shutdown: bool,
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
            early_boot_logs_path: None,
            spawn_getty: true,
            attach_ttys: true,
            socket: crate::config::default_socket_path().display().to_string(),
            machine_shutdown: true,
        }
    }
}

/// Options for `microinit supervise` (containers / embedded hosts).
#[must_use]
pub fn supervise_opts(
    console: String,
    paths: Paths,
    socket: String,
    log_to_files: bool,
) -> InitOpts {
    InitOpts {
        logs_tty: DEFAULT_LOGS_TTY.to_string(),
        init_logs_tty: DEFAULT_INIT_LOGS_TTY.to_string(),
        console,
        paths,
        skip_early_boot: true,
        require_early_boot: false,
        log_to_files,
        early_boot_logs_path: None,
        spawn_getty: false,
        attach_ttys: false,
        socket,
        machine_shutdown: false,
    }
}

pub fn run(opts: InitOpts) -> Result<()> {
    crate::log_bridge::install();
    // Resolve init-logs path early so pre-hub boot notes reach tty3 as well as stderr.
    let init_logs_preview = if opts.attach_ttys {
        Some(opts.init_logs_tty.as_str())
    } else {
        None
    };

    if let Err(e) = signals::install_handlers() {
        boot_note(
            init_logs_preview,
            &format!("warning: could not install signal handlers: {e}"),
        );
    }

    #[cfg(feature = "init")]
    let mut early_boot_out: Option<crate::early_boot::EarlyBootOutput> = None;

    #[cfg(feature = "init")]
    {
        if opts.skip_early_boot {
            boot_note(
                init_logs_preview,
                "skipping early-boot (--no-early-boot / supervise)",
            );
        } else {
            let (out, result) = crate::early_boot::run(
                &opts.paths,
                &opts.logs_tty,
                &opts.init_logs_tty,
                &opts.console,
            );
            match result {
                Ok(()) => {
                    boot_note(
                        init_logs_preview,
                        "early-boot finished; loading configuration from disk",
                    );
                    early_boot_out = Some(out);
                }
                Err(e) => {
                    boot_note(init_logs_preview, &format!("early-boot failed: {e}"));
                    if opts.require_early_boot {
                        persist_early_boot_logs(init_logs_preview, &opts, &out);
                        return Err(e);
                    }
                    early_boot_out = Some(out);
                    boot_note(
                        init_logs_preview,
                        "continuing without early-boot; loading configuration from disk",
                    );
                }
            }
        }
    }
    #[cfg(not(feature = "init"))]
    {
        let _ = opts.skip_early_boot;
        let _ = opts.require_early_boot;
        let _ = &opts.early_boot_logs_path;
        boot_note(
            init_logs_preview,
            "early-boot disabled (supervise-only / no-init build)",
        );
    }

    // Always (re)load JSON after early-boot: the script mounts `$DATA_DIR` and
    // may seed/update `microinit.json`, drop-ins, and the enabled-override.
    let mut cfg = match load_config_after_early_boot(&opts) {
        Ok(c) => c,
        Err(e) => {
            #[cfg(feature = "init")]
            if let Some(ref out) = early_boot_out {
                persist_early_boot_logs(init_logs_preview, &opts, out);
            }
            return Err(e);
        }
    };

    if let Err(e) = crate::otelenv::load_default() {
        boot_note(
            init_logs_preview,
            &format!("otel.env load failed (continuing): {e}"),
        );
    }

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

    let (svc_tty, init_tty) = if opts.attach_ttys {
        (Some(logs_tty.as_str()), Some(init_logs_tty.as_str()))
    } else {
        (None, None)
    };
    let hub = Arc::new(LogHub::new(cfg.logs.lines, svc_tty, init_tty, log_dir));
    crate::log_bridge::set_hub(hub.clone());
    // Boot phase: init lines → tty3 and stderr (see LogHub::boot_tee_stderr).
    let console = Arc::new(Console::open_with_hub(&opts.console, Some(hub.clone())));

    hub.emit_init(LogLevel::Info, "configuration loaded");
    #[cfg(feature = "init")]
    flush_early_boot_logs(&hub, &cfg, early_boot_out.as_ref(), opts.skip_early_boot);
    if opts.attach_ttys {
        hub.emit_init(
            LogLevel::Info,
            format!("service logs on {logs_tty}; init logs on {init_logs_tty}"),
        );
    } else {
        hub.emit_init(
            LogLevel::Info,
            "TTY attach disabled (supervise); logs via ring/IPC",
        );
    }
    if cfg.logs.log_to_files {
        if let Some(ref dir) = cfg.logs.dir {
            hub.emit_init(
                LogLevel::Info,
                format!("logToFiles enabled; writing under {dir}"),
            );
        }
    }

    let socket_path = cfg.socket.clone();
    let lines_default = cfg.logs.lines;
    let override_path = opts.paths.override_file.clone();
    let config_path = opts.paths.config.clone();
    let dropins_dir = opts.paths.dropins_dir.clone();

    let mode = if opts.machine_shutdown {
        crate::protocol::DaemonMode::Init
    } else {
        crate::protocol::DaemonMode::Supervise
    };
    let ipc_allow = cfg.resolved_ipc_allow()?;
    let supervisor = Supervisor::new(
        cfg,
        hub.clone(),
        console.clone(),
        override_path,
        config_path,
        dropins_dir,
        mode,
    );

    let sup = Arc::clone(&supervisor);
    let hub_ipc = hub.clone();
    ipc::serve(
        Path::new(&socket_path),
        Arc::new(move |req, stream| handle_ipc(req, stream, &sup, &hub_ipc, lines_default)),
        ipc_allow,
    )?;
    hub.emit_init(LogLevel::Info, format!("IPC listening on {socket_path}"));

    // Watch before boot: the control socket is already public, so a config write
    // that races with service start must still queue a reload for the main loop.
    let etc_dir = opts
        .paths
        .config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/data/etc"));
    let dropins_dir_watch = opts.paths.dropins_dir.clone();
    let paths_for_reload = opts.paths.clone();
    let socket_override = opts.socket.clone();
    let (reload_rx, _watch_stop) =
        match crate::config_watch::spawn(etc_dir, dropins_dir_watch, hub.clone()) {
            Ok(pair) => pair,
            Err(e) => {
                hub.emit_init(LogLevel::Warn, format!("config watch unavailable: {e}"));
                let (_tx, rx) = std::sync::mpsc::channel();
                (rx, Arc::new(std::sync::atomic::AtomicBool::new(true)))
            }
        };

    hub.emit_init(LogLevel::Info, "starting services");
    supervisor.boot()?;
    hub.emit_init(LogLevel::Info, "boot complete");

    #[cfg(feature = "otel")]
    {
        let otel_cfg = supervisor.open_telemetry();
        let _otel_stop = crate::otel::maybe_spawn(supervisor.clone(), otel_cfg, hub.clone());
    }
    #[cfg(not(feature = "otel"))]
    {
        if supervisor.open_telemetry().enable {
            hub.emit_init(
                LogLevel::Warn,
                "openTelemetry.enable=true but binary built without OpenTelemetry (`--no-default-features`)",
            );
        }
    }

    // After boot / before getty: lifecycle logs only on --init-logs-tty.
    hub.end_boot_tee();
    hub.emit_init(
        LogLevel::Info,
        "boot tee ended; further lifecycle logs only on init-logs-tty",
    );

    // Getty only for full init as PID 1 — never for supervise (also often PID 1 in containers).
    #[cfg(feature = "init")]
    {
        if opts.spawn_getty && std::process::id() == 1 {
            let console_dev = opts.console.clone();
            thread::spawn(move || getty_respawn(&console_dev));
        } else if !opts.spawn_getty {
            hub.emit_init(LogLevel::Info, "getty disabled (supervise)");
        } else {
            hub.emit_init(LogLevel::Info, "not PID 1; skipping getty respawn");
        }
    }
    #[cfg(not(feature = "init"))]
    {
        let _ = opts.spawn_getty;
        hub.emit_init(
            LogLevel::Info,
            "getty disabled (supervise-only / no-init build)",
        );
    }

    let machine_shutdown = opts.machine_shutdown;
    #[cfg(feature = "init")]
    let paths_for_unmount = opts.paths.clone();
    #[cfg(feature = "init")]
    let unmount_logs_tty = logs_tty.clone();
    #[cfg(feature = "init")]
    let unmount_init_logs_tty = init_logs_tty.clone();
    #[cfg(feature = "init")]
    let unmount_console = opts.console.clone();

    // Exits are published by the process-wide reaper thread started in Supervisor::boot.
    // Opportunistic reap here covers the window before that thread runs and orphans.
    let exits = global_exits();
    loop {
        let _ = signals::sigchld_pending();
        for (pid, code) in signals::reap_zombies() {
            exits.publish(pid.as_raw(), code);
        }

        while reload_rx.try_recv().is_ok() {
            match config::load_or_create_with_dropins(
                &paths_for_reload.config,
                &paths_for_reload.example,
                &paths_for_reload.override_file,
                &paths_for_reload.dropins_dir,
            ) {
                Ok(mut cfg) => {
                    cfg.socket = socket_override.clone();
                    if let Err(e) = supervisor.reload(cfg) {
                        hub.emit_init(LogLevel::Error, format!("config reload apply failed: {e}"));
                    }
                }
                Err(e) => {
                    hub.emit_init(
                        LogLevel::Warn,
                        format!("config reload ignored (keep old): {e}"),
                    );
                }
            }
        }

        if let Some(mode) = take_shutdown() {
            hub.emit_init(LogLevel::Info, format!("shutdown requested: {mode}"));
            supervisor.stop_all_ordered();
            let _ = std::fs::remove_file(&socket_path);

            // Supervise / Android: stop services only — no unmount script, no reboot(2).
            if !machine_shutdown {
                let _ = mode;
                nix::unistd::sync();
                hub.emit_init(LogLevel::Info, "supervise shutdown complete; exiting");
                std::process::exit(0);
            }

            #[cfg(feature = "init")]
            {
                // Late unmount after all services are stopped; failures must not
                // block reboot/poweroff (stuck umount would hang the board forever).
                hub.emit_init(LogLevel::Info, "running late-shutdown unmount");
                if let Err(e) = crate::unmount::run(
                    &paths_for_unmount,
                    &unmount_logs_tty,
                    &unmount_init_logs_tty,
                    &unmount_console,
                ) {
                    hub.emit_init(
                        LogLevel::Warn,
                        format!("unmount failed (continuing to {mode}): {e}"),
                    );
                }
                crate::shutdown::finalize(mode);
            }
            #[cfg(not(feature = "init"))]
            {
                // machine_shutdown without feature init: treat as supervise exit.
                let _ = mode;
                nix::unistd::sync();
                hub.emit_init(LogLevel::Info, "supervise shutdown complete; exiting");
                std::process::exit(0);
            }
        }

        thread::sleep(INIT_LOOP_SLEEP);
    }
}

/// Load config from disk after early-boot has had a chance to mount `$DATA_DIR`
/// and seed JSON. CLI socket always overrides the file.
fn load_config_after_early_boot(opts: &InitOpts) -> Result<crate::config::Config> {
    let mut cfg = config::load_or_create_with_dropins(
        &opts.paths.config,
        &opts.paths.example,
        &opts.paths.override_file,
        &opts.paths.dropins_dir,
    )?;
    cfg.socket = opts.socket.clone();
    Ok(cfg)
}

/// Persist the RAM-buffered early-boot capture after mounts have settled.
/// Never fails the boot: a missing/RO `logsPath` is a warning.
#[cfg(feature = "init")]
fn flush_early_boot_logs(
    hub: &LogHub,
    cfg: &crate::config::Config,
    captured: Option<&crate::early_boot::EarlyBootOutput>,
    skipped: bool,
) {
    if !cfg.early_boot.capture_logs {
        hub.emit_init(LogLevel::Info, "early-boot log capture disabled");
        return;
    }
    if skipped {
        hub.emit_init(
            LogLevel::Info,
            "early-boot log capture not applicable (supervise / --no-early-boot)",
        );
        return;
    }
    let Some(out) = captured else {
        hub.emit_init(
            LogLevel::Warn,
            "early-boot log capture enabled but nothing to write",
        );
        return;
    };
    if !out.captured {
        hub.emit_init(
            LogLevel::Warn,
            format!(
                "early-boot log capture enabled but stdio was not captured; skipping {}",
                cfg.early_boot.logs_path
            ),
        );
        return;
    }
    let path = Path::new(&cfg.early_boot.logs_path);
    match crate::early_boot::write_captured(path, out) {
        Ok(n) => hub.emit_init(
            LogLevel::Info,
            format!("early-boot logs written to {} ({n} lines)", path.display()),
        ),
        Err(e) => hub.emit_init(
            LogLevel::Warn,
            format!("early-boot logs not written to {}: {e}", path.display()),
        ),
    }
}

/// Best-effort persist when we will not reach the normal post-config flush
/// (required early-boot failed, or config load failed). Never creates a
/// default `microinit.json`.
#[cfg(feature = "init")]
fn persist_early_boot_logs(
    init_logs_preview: Option<&str>,
    opts: &InitOpts,
    out: &crate::early_boot::EarlyBootOutput,
) {
    if !out.captured {
        boot_note(
            init_logs_preview,
            "early-boot log capture: stdio was not captured; nothing to write",
        );
        return;
    }
    let Some(path) = fatal_early_boot_logs_path(opts) else {
        boot_note(
            init_logs_preview,
            "early-boot logs not written (no --early-boot-logs-path and captureLogs not enabled in an existing config)",
        );
        return;
    };
    match crate::early_boot::write_captured(&path, out) {
        Ok(n) => boot_note(
            init_logs_preview,
            &format!("early-boot logs written to {} ({n} lines)", path.display()),
        ),
        Err(e) => boot_note(
            init_logs_preview,
            &format!("early-boot logs not written to {}: {e}", path.display()),
        ),
    }
}

/// CLI path wins; otherwise the live config if it already exists; otherwise
/// the image overlay next to the base early-boot script. Does not seed JSON.
#[cfg(feature = "init")]
fn fatal_early_boot_logs_path(opts: &InitOpts) -> Option<PathBuf> {
    if let Some(p) = &opts.early_boot_logs_path {
        return Some(p.clone());
    }
    match config::peek_early_boot_capture(&opts.paths.config) {
        config::EarlyBootCapturePeek::Enabled(p) => return Some(p),
        config::EarlyBootCapturePeek::Disabled => return None,
        config::EarlyBootCapturePeek::Absent => {}
    }
    let image = opts.paths.early_boot.parent()?.join("microinit.json");
    match config::peek_early_boot_capture(&image) {
        config::EarlyBootCapturePeek::Enabled(p) => Some(p),
        _ => None,
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
            Err(e) => write_frame(stream, &error_response(&e))?,
        },
        Request::Describe { name, output } => match supervisor.describe(&name, output) {
            Ok(describe) => write_frame(
                stream,
                &Response::Describe {
                    describe: Box::new(describe),
                },
            )?,
            Err(e) => write_frame(stream, &error_response(&e))?,
        },
        Request::Info => {
            write_frame(
                stream,
                &Response::Info {
                    info: Box::new(supervisor.info()),
                },
            )?;
        }
        Request::Start { name, force } => {
            respond_start(stream, supervisor.start_service(&name, force))?;
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
                // Heartbeats keep Go/UI clients with idle read deadlines alive
                // when a service is healthy but quiet.
                const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);
                loop {
                    match rx.recv_timeout(HEARTBEAT) {
                        Ok(line) => {
                            if let Some(ref nme) = name {
                                if &line.service != nme {
                                    continue;
                                }
                            }
                            if write_frame(stream, &Response::Log { line }).is_err() {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if write_frame(stream, &Response::Heartbeat).is_err() {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            } else {
                write_frame(stream, &Response::Ok { message: None })?;
            }
        }
        Request::Watch { label_keys } => {
            let Some(sub) = supervisor.watch_subscribe(label_keys) else {
                write_frame(
                    stream,
                    &Response::Error {
                        message: format!(
                            "too many concurrent watch clients (max {MAX_WATCH_FOLLOWERS})"
                        ),
                        code: Some("busy".into()),
                    },
                )?;
                return Ok(());
            };
            let mut seen = 0u64;
            loop {
                match sub.wait_timeout(seen, WATCH_HEARTBEAT) {
                    WaitOutcome::Snapshot { gen, services } => {
                        seen = gen;
                        if write_frame(
                            stream,
                            &Response::List {
                                services: (*services).clone(),
                            },
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                    WaitOutcome::Timeout => {
                        if write_frame(stream, &Response::Heartbeat).is_err() {
                            break;
                        }
                    }
                }
            }
        }
        Request::Shutdown { mode } => {
            write_frame(stream, &Response::Ok { message: None })?;
            signals::request_shutdown(mode);
        }
    }
    Ok(())
}

fn respond_start(stream: &mut UnixStream, res: Result<String>) -> Result<()> {
    match res {
        Ok(message) => write_frame(
            stream,
            &Response::Ok {
                message: Some(message),
            },
        )?,
        Err(e) => write_frame(stream, &error_response(&e))?,
    }
    Ok(())
}

fn respond_result(stream: &mut UnixStream, res: Result<()>) -> Result<()> {
    match res {
        Ok(()) => write_frame(stream, &Response::Ok { message: None })?,
        Err(e) => write_frame(stream, &error_response(&e))?,
    }
    Ok(())
}

/// Build an IPC error response with the stable [Error::code] populated when
/// available, so clients can map on `code` instead of substring-matching
/// the human-readable message.
fn error_response(e: &crate::error::Error) -> Response {
    Response::Error {
        message: e.to_string(),
        code: e.code().map(|s| s.to_string()),
    }
}

#[cfg(feature = "init")]
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
#[cfg(feature = "init")]
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
