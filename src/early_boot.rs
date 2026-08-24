//! Early-boot script runner.
//!
//! Search order:
//! 1. override under the data root (`$DATA_DIR/etc/microinit/early-boot.sh`)
//! 2. `/etc/microinit/early-boot.sh`
//! 3. portable script embedded in this binary (`scripts/early-boot.sh`)
//!
//! stdout and stderr are teed live to this process's stderr (kernel console on
//! PID 1) as raw bytes and captured into a bounded RAM buffer. The buffer is
//! flushed to `earlyBoot.logsPath` only **after** the script exits, so a distro
//! overlay that remounts `$DATA_DIR` (NVMe migration) still writes to the final
//! mount.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use chrono::Utc;

use crate::config::Paths;
use crate::constants::{
    MAX_EARLY_BOOT_CAPTURE_BYTES, MAX_EARLY_BOOT_CAPTURE_LINES, MAX_EARLY_BOOT_LINE_BYTES,
};
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

/// Bounded capture of early-boot script stdout+stderr.
#[derive(Debug, Clone, Default)]
pub struct EarlyBootOutput {
    pub lines: Vec<String>,
    /// Lines evicted because the line or byte capture limit was hit.
    pub dropped: usize,
    /// False when the pipe could not be created and stdio was inherited.
    pub captured: bool,
    /// Script source label for the on-disk header (`path` or `embedded`).
    pub source: String,
    /// Child exit code, or `None` if the process was killed by a signal / wait failed.
    pub exit_code: Option<i32>,
    /// Terminating signal when [`Self::exit_code`] is `None` because of a signal.
    /// `None` together with `exit_code: None` means `wait` failed.
    pub signal: Option<i32>,
}

#[derive(Debug, Default)]
struct Collected {
    lines: Vec<String>,
    dropped: usize,
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
///
/// Does **not** create `$DATA_DIR/etc` beforehand: that path may sit on an
/// unmounted mountpoint; the script itself mounts `/data` and seeds configs.
/// Callers must load `microinit.json` **after** this returns successfully.
///
/// The [`EarlyBootOutput`] is populated even when the script fails, so a
/// `--allow-no-early-boot` boot can still persist the captured lines.
pub fn run(
    paths: &Paths,
    logs_tty: &str,
    init_logs_tty: &str,
    console: &str,
) -> (EarlyBootOutput, Result<()>) {
    match resolve_script(paths) {
        ScriptSource::Path(script) => run_script(&script, logs_tty, init_logs_tty, console),
        ScriptSource::Embedded => {
            eprintln!("microinit: using embedded early-boot.sh");
            run_script_bytes(EMBEDDED_EARLY_BOOT, logs_tty, init_logs_tty, console)
        }
    }
}

pub fn run_script(
    script: &Path,
    logs_tty: &str,
    init_logs_tty: &str,
    console: &str,
) -> (EarlyBootOutput, Result<()>) {
    let data_root = datadir::root();
    let mut cmd = Command::new("/bin/sh");
    cmd.arg(script)
        .env("MICROINIT_LOGS_TTY", logs_tty)
        .env("MICROINIT_INIT_LOGS_TTY", init_logs_tty)
        .env("MICROINIT_CONSOLE", console)
        .env(datadir::ENV_DATA_DIR, &data_root);
    run_command(cmd, script.display().to_string())
}

/// Run script content via `sh -s` (used for the embedded default).
pub fn run_script_bytes(
    script: &str,
    logs_tty: &str,
    init_logs_tty: &str,
    console: &str,
) -> (EarlyBootOutput, Result<()>) {
    let data_root = datadir::root();
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-s")
        .stdin(Stdio::piped())
        .env("MICROINIT_LOGS_TTY", logs_tty)
        .env("MICROINIT_INIT_LOGS_TTY", init_logs_tty)
        .env("MICROINIT_CONSOLE", console)
        .env(datadir::ENV_DATA_DIR, &data_root);
    let capture = try_stdio_capture(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                EarlyBootOutput {
                    source: "embedded".into(),
                    ..EarlyBootOutput::default()
                },
                Err(Error::Other(format!("failed to exec /bin/sh -s: {e}"))),
            );
        }
    };
    // Close the parent's copies of the capture write ends so the reader sees EOF
    // after the child exits. `Command` keeps the original Fds until dropped.
    drop(cmd);
    let captured = capture.is_some();
    let handle = start_capture(capture);
    {
        let write_err = match child.stdin.take() {
            Some(mut stdin) => stdin
                .write_all(script.as_bytes())
                .map_err(|e| Error::Other(format!("failed to write early-boot script: {e}")))
                .err(),
            None => Some(Error::Other("failed to open sh stdin".into())),
        };
        if let Some(e) = write_err {
            let _ = child.kill();
            let _ = child.wait();
            let collected = join_capture(handle);
            let out = output_from_collected("embedded".into(), collected, captured);
            return (out, Err(e));
        }
    }
    finish_wait(child, handle, captured, "embedded".into())
}

fn run_command(mut cmd: Command, source: String) -> (EarlyBootOutput, Result<()>) {
    let capture = try_stdio_capture(&mut cmd);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                EarlyBootOutput {
                    source: source.clone(),
                    ..EarlyBootOutput::default()
                },
                Err(Error::Other(format!("failed to exec {source}: {e}"))),
            );
        }
    };
    // Close the parent's copies of the capture write ends so the reader sees EOF
    // after the child exits. `Command` keeps the original Fds until dropped.
    // Omitting this drop leaves the pipe open forever and `join_capture` hangs.
    drop(cmd);
    let captured = capture.is_some();
    let handle = start_capture(capture);
    finish_wait(child, handle, captured, source)
}

/// Attach stdout and stderr to one pipe so interleaving is preserved. Returns
/// the parent read end, or `None` to inherit stdio (capture must never break boot).
fn try_stdio_capture(cmd: &mut Command) -> Option<io::PipeReader> {
    let (reader, writer) = io::pipe().ok()?;
    let writer2 = writer.try_clone().ok()?;
    cmd.stdout(Stdio::from(writer));
    cmd.stderr(Stdio::from(writer2));
    Some(reader)
}

fn start_capture(capture: Option<io::PipeReader>) -> Option<thread::JoinHandle<Collected>> {
    capture.map(|reader| thread::spawn(move || collect_lines(reader)))
}

fn join_capture(handle: Option<thread::JoinHandle<Collected>>) -> Collected {
    match handle {
        Some(h) => h.join().unwrap_or_default(),
        None => Collected::default(),
    }
}

fn finish_wait(
    mut child: std::process::Child,
    handle: Option<thread::JoinHandle<Collected>>,
    captured: bool,
    source: String,
) -> (EarlyBootOutput, Result<()>) {
    let status = child.wait();
    let collected = join_capture(handle);
    let mut out = output_from_collected(source, collected, captured);
    match status {
        Ok(st) => {
            out.exit_code = st.code();
            out.signal = st.signal();
            let result = match st.code() {
                Some(0) => Ok(()),
                Some(code) => Err(Error::EarlyBoot(code)),
                None => Err(Error::EarlyBoot(1)),
            };
            (out, result)
        }
        Err(e) => (
            out,
            Err(Error::Other(format!("failed to wait for early-boot: {e}"))),
        ),
    }
}

fn output_from_collected(source: String, collected: Collected, captured: bool) -> EarlyBootOutput {
    EarlyBootOutput {
        lines: collected.lines,
        dropped: collected.dropped,
        captured,
        source,
        exit_code: None,
        signal: None,
    }
}

/// Read stdout+stderr as bytes. Invalid UTF-8 must not stop the reader: that
/// would fill the pipe and deadlock the script (PID 1 hang). Line length and
/// total buffer size are enforced while reading so a line without `\n` cannot
/// OOM init. Raw chunks are teed to stderr unchanged (console stays a tty-like
/// byte stream even though the child sees a pipe).
fn collect_lines(reader: impl Read) -> Collected {
    let mut lines: VecDeque<String> = VecDeque::new();
    let mut dropped = 0usize;
    let mut captured_bytes = 0usize;
    let mut reader = BufReader::new(reader);
    let mut cur: Vec<u8> = Vec::with_capacity(256);
    let mut overflowed = false;

    loop {
        let consumed = {
            let chunk = match reader.fill_buf() {
                Ok([]) => break,
                Ok(c) => c,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let _ = io::stderr().write_all(chunk);
            let n = chunk.len();
            for &b in chunk {
                if b == b'\n' {
                    push_captured_line(&mut lines, &mut dropped, &mut captured_bytes, &cur);
                    cur.clear();
                    overflowed = false;
                } else if !overflowed {
                    if cur.len() >= MAX_EARLY_BOOT_LINE_BYTES {
                        overflowed = true;
                    } else {
                        cur.push(b);
                    }
                }
            }
            n
        };
        reader.consume(consumed);
    }
    if !cur.is_empty() {
        push_captured_line(&mut lines, &mut dropped, &mut captured_bytes, &cur);
    }
    Collected {
        lines: lines.into_iter().collect(),
        dropped,
    }
}

fn push_captured_line(
    lines: &mut VecDeque<String>,
    dropped: &mut usize,
    captured_bytes: &mut usize,
    raw: &[u8],
) {
    let msg = String::from_utf8_lossy(raw).into_owned();
    let add = msg.len();
    while !lines.is_empty()
        && (lines.len() >= MAX_EARLY_BOOT_CAPTURE_LINES
            || *captured_bytes + add > MAX_EARLY_BOOT_CAPTURE_BYTES)
    {
        if let Some(old) = lines.pop_front() {
            *captured_bytes = captured_bytes.saturating_sub(old.len());
            *dropped += 1;
        }
    }
    lines.push_back(msg);
    *captured_bytes += add;
}

fn exit_header(out: &EarlyBootOutput) -> String {
    match (out.exit_code, out.signal) {
        (Some(c), _) => c.to_string(),
        (None, Some(sig)) => format!("signal:{sig}"),
        (None, None) => "unknown".into(),
    }
}

/// Truncate-write captured early-boot lines to `path`. Best effort.
///
/// Called only after the script exits, so `path` resolves against the final
/// mount (NVMe migration may have replaced `/data` mid-script).
pub fn write_captured(path: &Path, out: &EarlyBootOutput) -> Result<usize> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
        }
    }
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|e| Error::io_at(path, e))?;

    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let exit = exit_header(out);
    writeln!(f, "# early-boot ts={ts} source={} exit={exit}", out.source)
        .map_err(|e| Error::io_at(path, e))?;
    if out.dropped > 0 {
        writeln!(f, "... {} earlier line(s) dropped", out.dropped)
            .map_err(|e| Error::io_at(path, e))?;
    }
    for line in &out.lines {
        writeln!(f, "{line}").map_err(|e| Error::io_at(path, e))?;
    }
    f.flush().map_err(|e| Error::io_at(path, e))?;
    // `/data` is typically mounted with commit=15; this file exists to explain a
    // boot that ended in a hard power cut, so page cache is not enough.
    f.sync_all().map_err(|e| Error::io_at(path, e))?;
    Ok(out.lines.len())
}
