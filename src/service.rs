//! Service process execution helpers.

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};

use crate::config::ServiceConfig;
use crate::constants::TERMINATE_POLL;
use crate::error::{Error, Result};

/// Default PATH for supervised processes. Kernel-started PID 1 often has no
/// PATH; without `/sbin` tools like `ip` and `dhclient` are invisible.
const DEFAULT_SERVICE_PATH: &str =
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

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
