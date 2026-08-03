//! Linux inotify-based configuration watcher (no polling).
//!
//! Watches `$DATA_DIR/etc/` for `microinit.json`, the enabled-override file, and
//! recursively watches `microinit.d/` when present. Debounces bursts of events
//! (atomic write+rename) before signaling a reload.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::{Error, Result};
use crate::logs::LogHub;
use crate::logs::INIT_SERVICE;
use crate::protocol::LogLevel;

const DEBOUNCE: Duration = Duration::from_millis(300);

/// Signal that configuration files may have changed.
pub struct ReloadSignal;

/// Filter path events relevant to microinit JSON config.
pub fn is_relevant_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    if name.ends_with('~') || name.ends_with(".swp") || name.ends_with(".tmp") {
        return false;
    }
    if name == "microinit.json"
        || name == "microinit.services.enabled-override.json"
        || name == "microinit.d"
    {
        return true;
    }
    path.extension().and_then(|s| s.to_str()) == Some("json")
}

fn event_paths(event: &Event) -> impl Iterator<Item = &PathBuf> {
    event.paths.iter()
}

/// Spawn an inotify watcher thread. Returns a receiver of debounce-coalesced reload signals.
pub fn spawn(
    etc_dir: PathBuf,
    dropins_dir: PathBuf,
    hub: Arc<LogHub>,
) -> Result<(Receiver<ReloadSignal>, Arc<AtomicBool>)> {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thr = Arc::clone(&stop);

    thread::Builder::new()
        .name("config-watch".into())
        .spawn(move || {
            if let Err(e) = watch_loop(etc_dir, dropins_dir, hub, tx, stop_thr) {
                eprintln!("microinit: config watcher stopped: {e}");
            }
        })
        .map_err(|e| Error::Other(e.to_string()))?;

    Ok((rx, stop))
}

fn watch_loop(
    etc_dir: PathBuf,
    dropins_dir: PathBuf,
    hub: Arc<LogHub>,
    reload_tx: Sender<ReloadSignal>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let (raw_tx, raw_rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<Event, notify::Error>| {
            let _ = raw_tx.send(res);
        },
        notify::Config::default(),
    )
    .map_err(|e| Error::Other(format!("inotify watcher: {e}")))?;

    // Always watch etc/ so late creation of microinit.d / config files is seen.
    if etc_dir.is_dir() {
        watcher
            .watch(&etc_dir, RecursiveMode::NonRecursive)
            .map_err(|e| Error::Other(format!("watch {}: {e}", etc_dir.display())))?;
    } else if let Some(parent) = etc_dir.parent() {
        let _ = std::fs::create_dir_all(&etc_dir);
        if etc_dir.is_dir() {
            let _ = watcher.watch(&etc_dir, RecursiveMode::NonRecursive);
        } else if parent.is_dir() {
            let _ = watcher.watch(parent, RecursiveMode::NonRecursive);
        }
    }

    let mut watching_dropins = false;
    if dropins_dir.is_dir() {
        if let Err(e) = watcher.watch(&dropins_dir, RecursiveMode::Recursive) {
            hub.emit(
                INIT_SERVICE,
                LogLevel::Warn,
                format!(
                    "config watch: cannot watch drop-ins {}: {e}",
                    dropins_dir.display()
                ),
            );
        } else {
            watching_dropins = true;
        }
    }

    hub.emit(
        INIT_SERVICE,
        LogLevel::Info,
        format!(
            "config watch active on {} (drop-ins {})",
            etc_dir.display(),
            if watching_dropins {
                "recursive"
            } else {
                "pending"
            }
        ),
    );

    let mut pending: Option<Instant> = None;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let timeout = pending
            .map(|t| {
                let elapsed = t.elapsed();
                if elapsed >= DEBOUNCE {
                    Duration::from_millis(0)
                } else {
                    DEBOUNCE - elapsed
                }
            })
            .unwrap_or(Duration::from_secs(1));

        match raw_rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                let relevant = matches!(
                    event.kind,
                    EventKind::Create(_)
                        | EventKind::Modify(_)
                        | EventKind::Remove(_)
                        | EventKind::Any
                ) && event_paths(&event).any(|p| is_relevant_path(p));

                if relevant {
                    // Late-create of microinit.d / dropins tree
                    if !watching_dropins
                        && dropins_dir.is_dir()
                        && watcher
                            .watch(&dropins_dir, RecursiveMode::Recursive)
                            .is_ok()
                    {
                        watching_dropins = true;
                        hub.emit(
                            INIT_SERVICE,
                            LogLevel::Info,
                            format!(
                                "config watch: now watching drop-ins {}",
                                dropins_dir.display()
                            ),
                        );
                    }
                    pending = Some(Instant::now());
                }
            }
            Ok(Err(e)) => {
                hub.emit(
                    INIT_SERVICE,
                    LogLevel::Warn,
                    format!("config watch error: {e}"),
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if pending.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
                    pending = None;
                    let _ = reload_tx.send(ReloadSignal);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}
