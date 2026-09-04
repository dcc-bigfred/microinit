//! Forward `dcc-daemon` (`log` crate) lines into [`LogHub`].
//!
//! microinit does not otherwise initialize a `log` backend, so watcher warnings
//! would be dropped.

use std::sync::{Arc, RwLock};

use crate::logs::{LogHub, INIT_SERVICE};
use crate::protocol::LogLevel;

static LOGGER: Bridge = Bridge;
static HUB: RwLock<Option<Arc<LogHub>>> = RwLock::new(None);

struct Bridge;

impl log::Log for Bridge {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info && metadata.target().starts_with("dcc_daemon")
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let msg = record.args().to_string();
        let level = match record.level() {
            log::Level::Error => LogLevel::Error,
            log::Level::Warn => LogLevel::Warn,
            _ => LogLevel::Info,
        };
        match HUB.read() {
            Ok(g) => {
                if let Some(hub) = g.as_ref() {
                    hub.emit(INIT_SERVICE, level, msg);
                    return;
                }
            }
            Err(p) => {
                if let Some(hub) = p.into_inner().as_ref() {
                    hub.emit(INIT_SERVICE, level, msg);
                    return;
                }
            }
        }
        eprintln!("microinit: {msg}");
    }

    fn flush(&self) {}
}

/// Install the process-wide `log` facade. Safe to call more than once.
pub fn install() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

/// Route subsequent `dcc_daemon` log lines to this hub.
pub fn set_hub(hub: Arc<LogHub>) {
    match HUB.write() {
        Ok(mut g) => *g = Some(hub),
        Err(p) => *p.into_inner() = Some(hub),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dcc_daemon_warn_reaches_hub() {
        install();
        let hub = Arc::new(LogHub::new(32, None, None, None));
        set_hub(hub.clone());
        log::warn!(target: "dcc_daemon::config::watch", "cannot watch /x");
        let lines = hub.snapshot_mixed(32);
        assert!(
            lines.iter().any(|l| l.msg.contains("cannot watch /x")),
            "{lines:?}"
        );
    }
}
