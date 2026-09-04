//! Linux inotify-based configuration watcher (no polling).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use dcc_daemon::config::{spawn_signal, PathFilter, WatchSpec};

use crate::error::{Error, Result};
use crate::logs::LogHub;
use crate::logs::INIT_SERVICE;
use crate::protocol::LogLevel;

const DEBOUNCE: Duration = Duration::from_millis(300);

/// Signal that configuration files may have changed.
pub type ReloadSignal = dcc_daemon::config::Reload;

fn microinit_filter() -> PathFilter {
    PathFilter::Any {
        extensions: vec!["json".into()],
        extra_names: vec![
            "microinit.json".into(),
            "microinit.services.enabled-override.json".into(),
            "microinit.d".into(),
        ],
        ignore_suffixes: Vec::new(),
    }
}

/// Filter path events relevant to microinit JSON config.
pub fn is_relevant_path(path: &Path) -> bool {
    dcc_daemon::config::is_relevant_path(path, &microinit_filter())
}

/// Spawn an inotify watcher thread. Returns a receiver of debounce-coalesced reload signals.
pub fn spawn(
    etc_dir: PathBuf,
    dropins_dir: PathBuf,
    hub: Arc<LogHub>,
) -> Result<(Receiver<ReloadSignal>, Arc<AtomicBool>)> {
    let specs = vec![
        WatchSpec {
            path: etc_dir.clone(),
            recursive: false,
            filter: microinit_filter(),
        },
        // Always listed: `dcc-daemon` late-attaches when `microinit.d/services`
        // appears after first boot.
        WatchSpec {
            path: dropins_dir.clone(),
            recursive: true,
            filter: microinit_filter(),
        },
    ];
    let pair = spawn_signal(specs, DEBOUNCE).map_err(|e| Error::Other(e.to_string()))?;
    hub.emit(
        INIT_SERVICE,
        LogLevel::Info,
        format!(
            "config watch active on {} (drop-ins {})",
            etc_dir.display(),
            if dropins_dir.is_dir() {
                "recursive"
            } else {
                "pending"
            }
        ),
    );
    Ok(pair)
}
