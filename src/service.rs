//! Service process execution helpers.

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::config::ServiceConfig;
use crate::constants::TERMINATE_POLL;
use crate::error::{Error, Result};

/// Default PATH for supervised processes. Kernel-started PID 1 often has no
/// PATH; without `/sbin` tools like `ip` and `dhclient` are invisible.
const DEFAULT_SERVICE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

fn build_shell_command(
    cmd: &str,
    cfg: &ServiceConfig,
    env_extra: &HashMap<String, String>,
) -> Command {
    let mut c = Command::new("/bin/sh");
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
    c
}

/// Spawn a shell command with service env/cwd; stdout/stderr piped for capture.
pub fn spawn_shell(cmd: &str, cfg: &ServiceConfig) -> Result<Child> {
    build_shell_command(cmd, cfg, &HashMap::new())
        .spawn()
        .map_err(|e| Error::Service(cfg.name.clone(), e.to_string()))
}

/// Run a shell command to completion (for stop/restart scripts that are short-lived).
pub fn run_shell(
    cmd: &str,
    cfg: &ServiceConfig,
    env_extra: &HashMap<String, String>,
) -> Result<i32> {
    let status = build_shell_command(cmd, cfg, env_extra)
        .status()
        .map_err(|e| Error::Service(cfg.name.clone(), e.to_string()))?;
    Ok(status.code().unwrap_or(1))
}

/// Like [`run_shell`], but discard stdout/stderr (liveness probes must stay cheap/quiet).
pub fn run_shell_quiet(cmd: &str, cfg: &ServiceConfig) -> Result<i32> {
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

    let mut child = build_shell_command(cmd, cfg, &HashMap::new())
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
