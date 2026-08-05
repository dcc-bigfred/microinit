//! Service process execution helpers.

use std::collections::HashMap;
use std::fs;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::config::ServiceConfig;
use crate::constants::TERMINATE_POLL;
use crate::error::{Error, Result};
use crate::protocol::RunningIdentity;

/// Shell used to run service `cmd` / probes / stop scripts.
#[cfg(target_os = "android")]
const SHELL: &str = "/system/bin/sh";
#[cfg(not(target_os = "android"))]
const SHELL: &str = "/bin/sh";

/// Default PATH for supervised processes. Kernel-started PID 1 often has no
/// PATH; without `/sbin` tools like `ip` and `dhclient` are invisible.
#[cfg(target_os = "android")]
const DEFAULT_SERVICE_PATH: &str =
    "/system/bin:/system/xbin:/vendor/bin:/system/sbin:/data/local/tmp";
#[cfg(not(target_os = "android"))]
const DEFAULT_SERVICE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

fn build_shell_command(
    cmd: &str,
    cfg: &ServiceConfig,
    env_extra: &HashMap<String, String>,
    #[cfg(not(target_os = "android"))] ident: Option<&crate::security::ResolvedIdentity>,
) -> Command {
    let mut c = Command::new(SHELL);
    c.arg("-c")
        .arg(cmd)
        .current_dir(&cfg.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Predictable PATH unless the service (or caller) overrides it.
    if !cfg.env.contains_key("PATH") && !env_extra.contains_key("PATH") {
        c.env("PATH", DEFAULT_SERVICE_PATH);
    }

    for (k, v) in &cfg.env {
        c.env(k, v);
    }
    for (k, v) in env_extra {
        c.env(k, v);
    }

    #[cfg(not(target_os = "android"))]
    if let Some(ident) = ident {
        // Passwd-derived identity env unless the service overrides them.
        if let Some(ref home) = ident.home {
            if !cfg.env.contains_key("HOME") && !env_extra.contains_key("HOME") {
                c.env("HOME", home);
            }
        }
        if let Some(ref user) = ident.username {
            if !cfg.env.contains_key("USER") && !env_extra.contains_key("USER") {
                c.env("USER", user);
            }
            if !cfg.env.contains_key("LOGNAME") && !env_extra.contains_key("LOGNAME") {
                c.env("LOGNAME", user);
            }
        }
        crate::security::attach_pre_exec(&mut c, ident);
    }

    c
}

/// Prefer the identity cached by [`Config::prepare_security`]; fall back to a
/// one-shot resolve for ad-hoc test configs that skip prepare.
#[cfg(not(target_os = "android"))]
fn resolve_sec(cfg: &ServiceConfig) -> Result<Option<crate::security::ResolvedIdentity>> {
    if cfg.security_context.is_none() {
        return Ok(None);
    }
    if let Some(ref cached) = cfg.resolved_security {
        return Ok(Some(cached.clone()));
    }
    match &cfg.security_context {
        Some(ctx) => crate::security::resolve(ctx),
        None => Ok(None),
    }
}

/// Spawn a shell command with service env/cwd; stdout/stderr piped for capture.
pub fn spawn_shell(cmd: &str, cfg: &ServiceConfig) -> Result<Child> {
    #[cfg(not(target_os = "android"))]
    let ident = resolve_sec(cfg)?;
    #[cfg(not(target_os = "android"))]
    let mut cmd_built = build_shell_command(cmd, cfg, &HashMap::new(), ident.as_ref());
    #[cfg(target_os = "android")]
    let mut cmd_built = build_shell_command(cmd, cfg, &HashMap::new());

    cmd_built
        .spawn()
        .map_err(|e| Error::Service(cfg.name.clone(), e.to_string()))
}

/// Run a shell command to completion (for stop/restart scripts that are short-lived).
pub fn run_shell(
    cmd: &str,
    cfg: &ServiceConfig,
    env_extra: &HashMap<String, String>,
) -> Result<i32> {
    #[cfg(not(target_os = "android"))]
    let ident = resolve_sec(cfg)?;
    #[cfg(not(target_os = "android"))]
    let status = build_shell_command(cmd, cfg, env_extra, ident.as_ref())
        .status()
        .map_err(|e| Error::Service(cfg.name.clone(), e.to_string()))?;
    #[cfg(target_os = "android")]
    let status = build_shell_command(cmd, cfg, env_extra)
        .status()
        .map_err(|e| Error::Service(cfg.name.clone(), e.to_string()))?;
    Ok(status.code().unwrap_or(1))
}

/// Like [`run_shell`], but discard stdout/stderr (liveness probes must stay cheap/quiet).
pub fn run_shell_quiet(cmd: &str, cfg: &ServiceConfig) -> Result<i32> {
    #[cfg(not(target_os = "android"))]
    let ident = resolve_sec(cfg)?;
    #[cfg(not(target_os = "android"))]
    let mut c = build_shell_command(cmd, cfg, &HashMap::new(), ident.as_ref());
    #[cfg(target_os = "android")]
    let mut c = build_shell_command(cmd, cfg, &HashMap::new());
    c.stdout(Stdio::null()).stderr(Stdio::null());
    let status = c
        .status()
        .map_err(|e| Error::Service(cfg.name.clone(), e.to_string()))?;
    Ok(status.code().unwrap_or(1))
}

/// Quiet shell command with a hard timeout; kills the process group on expiry.
///
/// Returns `Ok(None)` on timeout.
pub fn run_shell_quiet_timeout(
    cmd: &str,
    cfg: &ServiceConfig,
    timeout: Duration,
) -> Result<Option<i32>> {
    use std::thread;
    use std::time::Instant;

    #[cfg(not(target_os = "android"))]
    let ident = resolve_sec(cfg)?;
    #[cfg(not(target_os = "android"))]
    let mut cmd_built = build_shell_command(cmd, cfg, &HashMap::new(), ident.as_ref());
    #[cfg(target_os = "android")]
    let mut cmd_built = build_shell_command(cmd, cfg, &HashMap::new());

    let mut child = cmd_built
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Service(cfg.name.clone(), e.to_string()))?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status.code().unwrap_or(1))),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let pid = child.id() as i32;
                    terminate_pid(nix::unistd::Pid::from_raw(pid), 0);
                    let _ = child.wait();
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(Error::Service(cfg.name.clone(), e.to_string()));
            }
        }
    }
}

/// Read the real uid/gid of a live process from `/proc/<pid>/status`.
///
/// Returns `None` if the process is gone or procfs is unavailable. Name
/// resolution via passwd/group is best-effort.
#[must_use]
pub fn read_running_identity(pid: i32) -> Option<RunningIdentity> {
    let path = format!("/proc/{pid}/status");
    let data = fs::read_to_string(path).ok()?;
    let mut uid: Option<u32> = None;
    let mut gid: Option<u32> = None;
    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            // real, effective, saved, fs — take real
            if let Some(tok) = rest.split_whitespace().next() {
                uid = tok.parse().ok();
            }
        } else if let Some(rest) = line.strip_prefix("Gid:") {
            if let Some(tok) = rest.split_whitespace().next() {
                gid = tok.parse().ok();
            }
        }
    }
    let uid = uid?;
    let gid = gid?;

    let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .ok()
        .flatten()
        .map(|u| u.name);
    let group = nix::unistd::Group::from_gid(nix::unistd::Gid::from_raw(gid))
        .ok()
        .flatten()
        .map(|g| g.name);

    Some(RunningIdentity {
        uid,
        gid,
        user,
        group,
    })
}

/// Kill a process with SIGTERM, wait `grace_secs`, then SIGKILL if still alive.
///
/// `grace_secs == 0` means SIGTERM then immediate SIGKILL (no wait).
pub fn terminate_pid(pid: nix::unistd::Pid, grace_secs: u64) {
    use nix::sys::signal::{kill, Signal};
    use std::thread;
    use std::time::{Duration, Instant};

    let _ = kill(pid, Signal::SIGTERM);
    if grace_secs == 0 {
        let _ = kill(pid, Signal::SIGKILL);
        return;
    }

    let deadline = Instant::now() + Duration::from_secs(grace_secs);
    while Instant::now() < deadline {
        match kill(pid, None) {
            Err(nix::errno::Errno::ESRCH) => return,
            _ => thread::sleep(TERMINATE_POLL),
        }
    }
    let _ = kill(pid, Signal::SIGKILL);
}
