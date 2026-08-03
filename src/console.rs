//! Systemd-style console status on the boot tty.
//!
//! Boot `[ OK ]` / `[ FAIL ]` lines are also mirrored (plain text) to the init
//! log hub so `--init-logs-tty` shows the same boot progress.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::logs::LogHub;
use crate::protocol::LogLevel;
use crate::syncutil::mutex_lock;

const GREEN: &str = "\x1b[0;32m";
const RED: &str = "\x1b[0;31m";
const RESET: &str = "\x1b[0m";
const WIDTH: usize = 48;

/// ANSI green used in [ OK ] markers (for tests / tooling).
pub const ANSI_GREEN: &str = GREEN;
/// ANSI red used in [ FAIL ] markers (for tests / tooling).
pub const ANSI_RED: &str = RED;

pub struct Console {
    out: Mutex<Box<dyn Write + Send>>,
    /// Optional hub for mirroring boot status onto `--init-logs-tty` (no stderr tee).
    hub: Option<Arc<LogHub>>,
}

impl Console {
    pub fn open(path: &str) -> Self {
        Self::open_with_hub(path, None)
    }

    pub fn open_with_hub(path: &str, hub: Option<Arc<LogHub>>) -> Self {
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .ok()
            .map(|f| Box::new(f) as Box<dyn Write + Send>);
        let out = file.unwrap_or_else(|| Box::new(std::io::stderr()) as Box<dyn Write + Send>);
        Self {
            out: Mutex::new(out),
            hub,
        }
    }

    pub fn stderr() -> Self {
        Self {
            out: Mutex::new(Box::new(std::io::stderr())),
            hub: None,
        }
    }

    /// In-memory console for unit tests.
    pub fn from_writer(w: Box<dyn Write + Send>) -> Self {
        Self {
            out: Mutex::new(w),
            hub: None,
        }
    }

    pub fn info(&self, msg: &str) {
        {
            let mut w = mutex_lock(&self.out);
            let _ = writeln!(w, "microinit: {msg}");
            let _ = w.flush();
        }
        if let Some(ref hub) = self.hub {
            hub.emit_init_console_mirror(LogLevel::Info, msg);
        }
    }

    pub fn starting(&self, name: &str) {
        {
            let mut w = mutex_lock(&self.out);
            let _ = write!(w, "  Starting {name}...");
            let _ = w.flush();
        }
        if let Some(ref hub) = self.hub {
            hub.emit_init_console_mirror(LogLevel::Info, format!("Starting {name}..."));
        }
    }

    pub fn ok(&self, name: &str) {
        self.status(name, true);
    }

    pub fn fail(&self, name: &str) {
        self.status(name, false);
    }

    fn status(&self, name: &str, ok: bool) {
        let pad = if name.len() < WIDTH {
            ".".repeat(WIDTH - name.len())
        } else {
            String::new()
        };
        let (tag, color) = if ok {
            ("[  OK  ]", GREEN)
        } else {
            ("[ FAIL ]", RED)
        };
        {
            let mut w = mutex_lock(&self.out);
            let _ = writeln!(w, "\r  {name}{pad} {color}{tag}{RESET}");
            let _ = w.flush();
        }
        if let Some(ref hub) = self.hub {
            let plain = if ok {
                format!("{name}: OK")
            } else {
                format!("{name}: FAIL")
            };
            hub.emit_init_console_mirror(if ok { LogLevel::Info } else { LogLevel::Error }, plain);
        }
    }
}
