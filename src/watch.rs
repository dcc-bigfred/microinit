//! Service-list watch fan-out for IPC `{type: watch}`.
//!
//! Memory profile: **allocation-conscious**. Follower slots allocate at
//! subscribe time. The supervisor `set_state` path is a single atomic load
//! when no followers exist (no snapshot clone).
//!
//! Each follower holds a coalescing slot of one snapshot. Rapid state
//! changes collapse to the latest view; a slow client is never more than
//! one snapshot behind and never grows an unbounded queue.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::constants::MAX_WATCH_FOLLOWERS;
use crate::labels::has_keys;
use crate::protocol::ServiceStatus;
use crate::syncutil::mutex_lock;

/// Outcome of waiting on a [`WatchSub`].
#[derive(Debug)]
pub enum WaitOutcome {
    /// Newer snapshot than the generation the caller last consumed.
    Snapshot {
        gen: u64,
        services: Arc<Vec<ServiceStatus>>,
    },
    /// `timeout` elapsed with no generation change.
    Timeout,
}

struct Slot {
    gen: u64,
    snapshot: Option<Arc<Vec<ServiceStatus>>>,
}

struct Follower {
    keys: Vec<String>,
    slot: Mutex<Slot>,
    cv: Condvar,
}

struct WatchHubInner {
    followers: Mutex<Vec<Arc<Follower>>>,
    count: AtomicUsize,
}

/// Shared watch fan-out owned by the supervisor.
pub struct WatchHub {
    inner: Arc<WatchHubInner>,
}

/// Live subscription. Dropping unregisters the follower.
pub struct WatchSub {
    follower: Arc<Follower>,
    hub: Arc<WatchHubInner>,
}

impl WatchHub {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(WatchHubInner {
                followers: Mutex::new(Vec::new()),
                count: AtomicUsize::new(0),
            }),
        }
    }

    /// Number of live followers. Cheap; used to skip snapshot clones.
    #[must_use]
    pub fn follower_count(&self) -> usize {
        self.inner.count.load(Ordering::SeqCst)
    }

    /// Subscribe. Returns `None` if [`MAX_WATCH_FOLLOWERS`] is reached.
    ///
    /// # Allocation
    ///
    /// Allocates one follower slot. Does not clone the service list.
    pub fn try_subscribe(&self, label_keys: Vec<String>) -> Option<WatchSub> {
        let mut followers = mutex_lock(&self.inner.followers);
        if followers.len() >= MAX_WATCH_FOLLOWERS {
            return None;
        }
        debug_assert!(followers.len() < MAX_WATCH_FOLLOWERS);
        let follower = Arc::new(Follower {
            keys: label_keys,
            slot: Mutex::new(Slot {
                gen: 0,
                snapshot: None,
            }),
            cv: Condvar::new(),
        });
        followers.push(Arc::clone(&follower));
        self.inner.count.store(followers.len(), Ordering::SeqCst);
        Some(WatchSub {
            follower,
            hub: Arc::clone(&self.inner),
        })
    }

    /// Publish a full unfiltered snapshot. No-op when there are no followers.
    ///
    /// Per follower, the list is filtered by `label_keys`. A frame is queued
    /// only when `(name, state, labels)` changed (pid / restarts ignored).
    ///
    /// # Allocation
    ///
    /// When `follower_count() == 0`, this function performs no heap
    /// allocation. Otherwise it clones matching [`ServiceStatus`] rows into
    /// one `Arc<Vec<_>>` per follower whose view changed.
    pub fn publish(&self, services: Vec<ServiceStatus>) {
        if self.inner.count.load(Ordering::SeqCst) == 0 {
            return;
        }
        let followers = mutex_lock(&self.inner.followers);
        for follower in followers.iter() {
            let filtered = filter_snapshot(&services, &follower.keys);
            debug_assert!(filtered.len() <= services.len());
            let mut slot = mutex_lock(&follower.slot);
            if let Some(ref old) = slot.snapshot {
                if views_eq(old, &filtered) {
                    continue;
                }
            }
            slot.gen = slot.gen.wrapping_add(1);
            slot.snapshot = Some(Arc::new(filtered));
            follower.cv.notify_one();
        }
    }
}

impl Default for WatchHub {
    fn default() -> Self {
        Self::new()
    }
}

impl WatchSub {
    /// Block until a snapshot newer than `seen_gen` is published, or `timeout`.
    ///
    /// Generation `0` means the caller has not consumed anything yet; the
    /// first publish after subscribe unblocks immediately.
    pub fn wait_timeout(&self, seen_gen: u64, timeout: Duration) -> WaitOutcome {
        let deadline = Instant::now() + timeout;
        let mut slot = mutex_lock(&self.follower.slot);
        loop {
            if slot.gen != seen_gen {
                if let Some(ref snap) = slot.snapshot {
                    return WaitOutcome::Snapshot {
                        gen: slot.gen,
                        services: Arc::clone(snap),
                    };
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return WaitOutcome::Timeout;
            }
            let remaining = deadline.saturating_duration_since(now);
            let (guard, result) = self
                .follower
                .cv
                .wait_timeout(slot, remaining)
                .unwrap_or_else(|e| e.into_inner());
            slot = guard;
            if result.timed_out() && slot.gen == seen_gen {
                return WaitOutcome::Timeout;
            }
        }
    }
}

impl Drop for WatchSub {
    fn drop(&mut self) {
        let mut followers = mutex_lock(&self.hub.followers);
        followers.retain(|f| !Arc::ptr_eq(f, &self.follower));
        self.hub.count.store(followers.len(), Ordering::SeqCst);
        self.follower.cv.notify_all();
    }
}

fn filter_snapshot(services: &[ServiceStatus], keys: &[String]) -> Vec<ServiceStatus> {
    if keys.is_empty() {
        return services.to_vec();
    }
    services
        .iter()
        .filter(|s| has_keys(&s.labels, keys))
        .cloned()
        .collect()
}

/// Equality for watch coalescing: name + state + labels only.
fn views_eq(a: &[ServiceStatus], b: &[ServiceStatus]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| x.name == y.name && x.state == y.state && x.labels == y.labels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ServiceState;
    use std::collections::BTreeMap;

    fn status(
        name: &str,
        state: ServiceState,
        pid: Option<i32>,
        labels: BTreeMap<String, String>,
    ) -> ServiceStatus {
        ServiceStatus {
            name: name.into(),
            state,
            pid,
            restarts: 0,
            liveness_failures: 0,
            enabled: true,
            labels,
        }
    }

    #[test]
    fn views_eq_ignores_pid() {
        let a = [status("a", ServiceState::Running, Some(1), BTreeMap::new())];
        let b = [status("a", ServiceState::Running, Some(2), BTreeMap::new())];
        assert!(views_eq(&a, &b));
        let c = [status("a", ServiceState::Stopped, Some(2), BTreeMap::new())];
        assert!(!views_eq(&a, &c));
    }
}
