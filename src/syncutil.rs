//! Small sync helpers for PID 1 (never panic on poisoned mutexes).

use std::sync::{Mutex, MutexGuard};

/// Lock a mutex; on poison, recover the guard so PID 1 keeps running.
#[inline]
pub fn mutex_lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
