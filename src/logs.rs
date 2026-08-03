//! Log capture: ring buffers, dual-TTY fan-out, optional file sink.
//!
//! - Service stdout/stderr → `--logs-tty` (default `/dev/tty2`)
//! - microinit operational lines (`service == "microinit"`) → `--init-logs-tty` (default `/dev/tty3`)
//! - During boot, init lines are also teed to stderr so the console matches tty3;
//!   after [`LogHub::end_boot_tee`] lifecycle logs stay on tty3 only.
//! - File append (`log_dir/<service>.log`) only when the hub is constructed with a
//!   directory — gated by config `logs.logToFiles` (default off).
//!
//! Memory profile: **allocation-conscious**. Ring buffers are fixed-capacity;
//! follower fan-out is capped.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::Utc;

use crate::constants::MAX_LOG_FOLLOWERS;
use crate::protocol::{LogLevel, LogLine};
use crate::syncutil::mutex_lock;

/// Service name used for microinit's own operational log lines.
pub const INIT_SERVICE: &str = "microinit";

#[derive(Debug)]
pub struct RingBuffer {
    capacity: usize,
    lines: VecDeque<LogLine>,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            lines: VecDeque::with_capacity(capacity.min(64)),
        }
    }

    pub fn push(&mut self, line: LogLine) {
        debug_assert!(self.capacity >= 1);
        while self.lines.len() >= self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn last_n(&self, n: usize) -> Vec<LogLine> {
        let start = self.lines.len().saturating_sub(n);
        self.lines.iter().skip(start).cloned().collect()
    }

    pub fn all(&self) -> Vec<LogLine> {
        self.lines.iter().cloned().collect()
    }
}

fn open_tty(path: &str) -> Option<File> {
    OpenOptions::new()
        .write(true)
        .open(path)
        .ok()
        .or_else(|| File::create(path).ok())
}

fn write_tty_line(tty: &Mutex<Option<File>>, line: &str) {
    let mut guard = mutex_lock(tty);
    if let Some(ref mut f) = *guard {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

/// Shared log hub used by supervisor and IPC.
pub struct LogHub {
    capacity: usize,
    per_service: Mutex<HashMap<String, RingBuffer>>,
    mixed: Mutex<RingBuffer>,
    /// Mixed service stdout/stderr (default tty2).
    service_tty: Mutex<Option<File>>,
    /// microinit operational logs (default tty3).
    init_tty: Mutex<Option<File>>,
    log_dir: Option<PathBuf>,
    followers: Mutex<Vec<std::sync::mpsc::Sender<LogLine>>>,
    /// When true, init-service lines are also written to stderr (boot phase).
    boot_tee_stderr: AtomicBool,
}

impl LogHub {
    pub fn new(
        capacity: usize,
        service_tty: Option<&str>,
        init_tty: Option<&str>,
        log_dir: Option<PathBuf>,
    ) -> Self {
        if let Some(ref dir) = log_dir {
            let _ = std::fs::create_dir_all(dir);
        }
        Self {
            capacity,
            per_service: Mutex::new(HashMap::new()),
            mixed: Mutex::new(RingBuffer::new(capacity)),
            service_tty: Mutex::new(service_tty.and_then(open_tty)),
            init_tty: Mutex::new(init_tty.and_then(open_tty)),
            log_dir,
            followers: Mutex::new(Vec::new()),
            // Tee init logs to stderr until boot finishes / getty takes the console.
            boot_tee_stderr: AtomicBool::new(true),
        }
    }

    /// Stop mirroring init logs to stderr. Call after the boot sequence completes
    /// so runtime lifecycle (`start`/`stop`/…) appears only on `--init-logs-tty`.
    pub fn end_boot_tee(&self) {
        self.boot_tee_stderr.store(false, Ordering::SeqCst);
    }

    #[must_use]
    pub fn boot_tee_enabled(&self) -> bool {
        self.boot_tee_stderr.load(Ordering::SeqCst)
    }

    /// Subscribe to live lines. Returns `None` if the follower cap is reached.
    pub fn try_subscribe(&self) -> Option<std::sync::mpsc::Receiver<LogLine>> {
        let mut followers = mutex_lock(&self.followers);
        if followers.len() >= MAX_LOG_FOLLOWERS {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        followers.push(tx);
        Some(rx)
    }

    /// Subscribe to live lines, dropping the oldest follower if at capacity.
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<LogLine> {
        let mut followers = mutex_lock(&self.followers);
        while followers.len() >= MAX_LOG_FOLLOWERS {
            let _ = followers.remove(0);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        followers.push(tx);
        rx
    }

    pub fn emit(&self, service: &str, level: LogLevel, msg: impl AsRef<str>) {
        self.publish(LogLine {
            ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            service: service.to_string(),
            level,
            msg: msg.as_ref().to_string(),
        });
    }

    /// Emit an init/operational line. Same as `emit(INIT_SERVICE, …)`.
    pub fn emit_init(&self, level: LogLevel, msg: impl AsRef<str>) {
        self.emit(INIT_SERVICE, level, msg);
    }

    /// Record an init line on tty3 / ring / files, but never tee to stderr.
    /// Used when the console already displayed the message (avoid duplicates).
    pub fn emit_init_console_mirror(&self, level: LogLevel, msg: impl AsRef<str>) {
        self.publish_inner(
            LogLine {
                ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                service: INIT_SERVICE.to_string(),
                level,
                msg: msg.as_ref().to_string(),
            },
            false,
        );
    }

    pub fn publish(&self, line: LogLine) {
        let tee = line.service == INIT_SERVICE && self.boot_tee_enabled();
        self.publish_inner(line, tee);
    }

    fn publish_inner(&self, line: LogLine, tee_stderr: bool) {
        {
            let mut map = mutex_lock(&self.per_service);
            let buf = map
                .entry(line.service.clone())
                .or_insert_with(|| RingBuffer::new(self.capacity));
            buf.push(line.clone());
        }
        mutex_lock(&self.mixed).push(line.clone());

        let formatted = format!("[{}] {}: {}", line.ts, line.service, line.msg);
        if line.service == INIT_SERVICE {
            write_tty_line(&self.init_tty, &formatted);
            if tee_stderr {
                let _ = writeln!(std::io::stderr(), "{formatted}");
                let _ = std::io::stderr().flush();
            }
        } else {
            write_tty_line(&self.service_tty, &formatted);
        }

        if let Some(ref dir) = self.log_dir {
            let path = dir.join(format!("{}.log", line.service));
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "[{}] {}: {}", line.ts, line.level, line.msg);
            }
        }

        let mut followers = mutex_lock(&self.followers);
        followers.retain(|tx| tx.send(line.clone()).is_ok());
    }

    pub fn snapshot_service(&self, name: &str, lines: usize) -> Vec<LogLine> {
        mutex_lock(&self.per_service)
            .get(name)
            .map(|b| b.last_n(lines))
            .unwrap_or_default()
    }

    pub fn snapshot_mixed(&self, lines: usize) -> Vec<LogLine> {
        mutex_lock(&self.mixed).last_n(lines)
    }
}

/// Write a boot-time note to stderr and optionally to an init-logs TTY path
/// before the [`LogHub`] exists.
pub fn boot_note(init_logs_tty: Option<&str>, msg: &str) {
    let line = format!("microinit: {msg}");
    let _ = writeln!(std::io::stderr(), "{line}");
    if let Some(path) = init_logs_tty {
        if let Some(mut f) = open_tty(path) {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

/// Spawn a reader thread that forwards lines from a pipe into the hub.
pub fn capture_stream(
    hub: Arc<LogHub>,
    service: String,
    level: LogLevel,
    stream: impl std::io::Read + Send + 'static,
) {
    thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(msg) => hub.emit(&service, level, msg),
                Err(_) => break,
            }
        }
    });
}
