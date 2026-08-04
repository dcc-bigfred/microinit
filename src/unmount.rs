//! Late-shutdown unmount script runner.
//!
//! Search order (same pattern as early-boot):
//! 1. override under the data root (`$DATA_DIR/etc/microinit/unmount.sh`)
//! 2. `/etc/microinit/unmount.sh`
//! 3. portable script embedded in this binary (`scripts/unmount.sh`)
//!
//! Failures are reported to the caller; PID 1 should log and continue to
//! `reboot(2)` rather than hang forever on a stuck umount.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::Paths;
use crate::datadir;
use crate::error::{Error, Result};

/// Portable unmount script baked into the binary.
pub const EMBEDDED_UNMOUNT: &str = include_str!("../scripts/unmount.sh");

/// Where the unmount script comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptSource {
    Path(PathBuf),
    Embedded,
}

/// Resolve unmount script: data-root override, then `/etc`, then embedded.
#[must_use]
pub fn resolve_script(paths: &Paths) -> ScriptSource {
    if paths.unmount_override.is_file() {
        ScriptSource::Path(paths.unmount_override.clone())
    } else if paths.unmount.is_file() {
        ScriptSource::Path(paths.unmount.clone())
    } else {
        ScriptSource::Embedded
    }
}

/// Run late-shutdown unmount (on-disk override/base, or embedded default).
pub fn run(paths: &Paths, logs_tty: &str, init_logs_tty: &str, console: &str) -> Result<()> {
    match resolve_script(paths) {
        ScriptSource::Path(script) => run_script(&script, logs_tty, init_logs_tty, console),
        ScriptSource::Embedded => {
            eprintln!("microinit: using embedded unmount.sh");
            run_script_bytes(EMBEDDED_UNMOUNT, logs_tty, init_logs_tty, console)
        }
    }
}

pub fn run_script(script: &Path, logs_tty: &str, init_logs_tty: &str, console: &str) -> Result<()> {
    let data_root = datadir::root();
    let status = Command::new("/bin/sh")
        .arg(script)
        .env("MICROINIT_LOGS_TTY", logs_tty)
        .env("MICROINIT_INIT_LOGS_TTY", init_logs_tty)
        .env("MICROINIT_CONSOLE", console)
        .env(datadir::ENV_DATA_DIR, &data_root)
        .status()
        .map_err(|e| Error::Other(format!("failed to exec {}: {e}", script.display())))?;

    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(Error::Unmount(code)),
        None => Err(Error::Unmount(1)),
    }
}

/// Run script content via `sh -s` (used for the embedded default).
pub fn run_script_bytes(
    script: &str,
    logs_tty: &str,
    init_logs_tty: &str,
    console: &str,
) -> Result<()> {
    let data_root = datadir::root();
    let mut child = Command::new("/bin/sh")
        .arg("-s")
        .stdin(Stdio::piped())
        .env("MICROINIT_LOGS_TTY", logs_tty)
        .env("MICROINIT_INIT_LOGS_TTY", init_logs_tty)
        .env("MICROINIT_CONSOLE", console)
        .env(datadir::ENV_DATA_DIR, &data_root)
        .spawn()
        .map_err(|e| Error::Other(format!("failed to exec /bin/sh -s: {e}")))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Other("failed to open sh stdin".into()))?;
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| Error::Other(format!("failed to write unmount script: {e}")))?;
    }

    match child.wait().map(|s| s.code()) {
        Ok(Some(0)) => Ok(()),
        Ok(Some(code)) => Err(Error::Unmount(code)),
        Ok(None) => Err(Error::Unmount(1)),
        Err(e) => Err(Error::Other(format!("failed to wait for unmount: {e}"))),
    }
}
