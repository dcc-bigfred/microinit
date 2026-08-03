//! Early-boot script runner.
//!
//! Search order:
//! 1. override under the data root (`$BIGFRED_DATA_DIR/etc/microinit/early-boot.sh`)
//! 2. `/etc/microinit/early-boot.sh`
//! 3. portable script embedded in this binary (`scripts/early-boot.sh`)

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::Paths;
use crate::datadir;
use crate::error::{Error, Result};

/// Portable early-boot script baked into the binary.
pub const EMBEDDED_EARLY_BOOT: &str = include_str!("../scripts/early-boot.sh");

/// Where the early-boot script comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptSource {
    /// Script on the filesystem.
    Path(PathBuf),
    /// [`EMBEDDED_EARLY_BOOT`] when no on-disk script exists.
    Embedded,
}

/// Resolve early-boot script: data-root override, then `/etc`, then embedded.
#[must_use]
pub fn resolve_script(paths: &Paths) -> ScriptSource {
    if paths.early_boot_override.is_file() {
        ScriptSource::Path(paths.early_boot_override.clone())
    } else if paths.early_boot.is_file() {
        ScriptSource::Path(paths.early_boot.clone())
    } else {
        ScriptSource::Embedded
    }
}

/// Run early-boot (on-disk override/base, or embedded default).
pub fn run(paths: &Paths, logs_tty: &str, init_logs_tty: &str, console: &str) -> Result<()> {
    if let Some(parent) = paths.config.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match resolve_script(paths) {
        ScriptSource::Path(script) => run_script(&script, logs_tty, init_logs_tty, console),
        ScriptSource::Embedded => {
            eprintln!("microinit: using embedded early-boot.sh");
            run_script_bytes(EMBEDDED_EARLY_BOOT, logs_tty, init_logs_tty, console)
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
        .env(datadir::ENV_PRIMARY, &data_root)
        .env(datadir::ENV_FALLBACK, &data_root)
        .status()
        .map_err(|e| Error::Other(format!("failed to exec {}: {e}", script.display())))?;

    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(Error::EarlyBoot(code)),
        None => Err(Error::EarlyBoot(1)),
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
        .env(datadir::ENV_PRIMARY, &data_root)
        .env(datadir::ENV_FALLBACK, &data_root)
        .spawn()
        .map_err(|e| Error::Other(format!("failed to exec /bin/sh -s: {e}")))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Other("failed to open sh stdin".into()))?;
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| Error::Other(format!("failed to write early-boot script: {e}")))?;
    }

    match child.wait().map(|s| s.code()) {
        Ok(Some(0)) => Ok(()),
        Ok(Some(code)) => Err(Error::EarlyBoot(code)),
        Ok(None) => Err(Error::EarlyBoot(1)),
        Err(e) => Err(Error::Other(format!("failed to wait for early-boot: {e}"))),
    }
}
