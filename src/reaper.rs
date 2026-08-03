//! Central child-exit registry for PID 1.
//!
//! Monitors must not call `Child::wait` while any thread calls `waitpid(-1)`.
//! Instead: forget the `Child` after spawn, reap centrally, publish here.
//!
//! A single process-wide registry (and one reaper thread) avoids races when
//! multiple supervisors exist in the same process (unit tests).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::constants::{CTL_POLL, MAX_EXIT_REGISTRY_ENTRIES};
use crate::signals::reap_zombies;
use crate::syncutil::mutex_lock;

#[derive(Debug, Default)]
pub struct ExitRegistry {
    exits: Mutex<HashMap<i32, i32>>,
    cv: Condvar,
}

impl ExitRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an exit observed by a central reaper.
    ///
    /// The map is capacity-bounded ([`MAX_EXIT_REGISTRY_ENTRIES`]): if full, an
    /// arbitrary existing entry is dropped so PID 1 memory stays finite.
    pub fn publish(&self, pid: i32, code: i32) {
        let mut map = mutex_lock(&self.exits);
        if map.len() >= MAX_EXIT_REGISTRY_ENTRIES && !map.contains_key(&pid) {
            if let Some(&victim) = map.keys().next() {
                map.remove(&victim);
            }
        }
        debug_assert!(map.len() < MAX_EXIT_REGISTRY_ENTRIES || map.contains_key(&pid));
        map.insert(pid, code);
        self.cv.notify_all();
    }

    /// Non-blocking take of a published exit code.
    pub fn take(&self, pid: i32) -> Option<i32> {
        mutex_lock(&self.exits).remove(&pid)
    }

    /// Wait until `pid` exits or `timeout` elapses.
    pub fn wait_take(&self, pid: i32, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        let mut map = mutex_lock(&self.exits);
        loop {
            if let Some(code) = map.remove(&pid) {
                return Some(code);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, result) = self
                .cv
                .wait_timeout(map, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            map = guard;
            if result.timed_out() {
                return map.remove(&pid);
            }
        }
    }
}

static GLOBAL_EXITS: OnceLock<Arc<ExitRegistry>> = OnceLock::new();
static REAPER_STARTED: AtomicBool = AtomicBool::new(false);

/// Process-wide exit registry shared by all supervisors and the init loop.
pub fn global_exits() -> Arc<ExitRegistry> {
    GLOBAL_EXITS
        .get_or_init(|| Arc::new(ExitRegistry::new()))
        .clone()
}

/// Ensure a single background `waitpid(-1)` thread publishes into [`global_exits`].
pub fn ensure_reaper_thread() {
    if REAPER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let exits = global_exits();
    let _ = thread::Builder::new()
        .name("microinit-reaper".into())
        .spawn(move || loop {
            for (pid, code) in reap_zombies() {
                exits.publish(pid.as_raw(), code);
            }
            thread::sleep(CTL_POLL);
        });
}
