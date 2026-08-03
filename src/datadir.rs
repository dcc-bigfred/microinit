//! Persistent data root resolution (aligned with BigFred `datadir`).
//!
//! Priority: `BIGFRED_DATA_DIR`, then `DATA_DIR`, then `/data` (hub default).
//! Only absolute paths are accepted from the environment; relative values are
//! ignored so misconfiguration cannot silently redirect data under the process
//! working directory.

use std::path::{Path, PathBuf};

/// Preferred env var (same name as BigFred / Android).
pub const ENV_PRIMARY: &str = "BIGFRED_DATA_DIR";
/// Fallback when [`ENV_PRIMARY`] is unset.
pub const ENV_FALLBACK: &str = "DATA_DIR";
/// Hub image default.
pub const DEFAULT_ROOT: &str = "/data";

/// Returns the persistent data directory.
#[must_use]
pub fn root() -> PathBuf {
    if let Some(v) = root_from_env(ENV_PRIMARY) {
        return v;
    }
    if let Some(v) = root_from_env(ENV_FALLBACK) {
        return v;
    }
    PathBuf::from(DEFAULT_ROOT)
}

fn root_from_env(name: &str) -> Option<PathBuf> {
    let v = std::env::var_os(name)?;
    if v.is_empty() {
        return None;
    }
    let p = PathBuf::from(v);
    if p.is_absolute() {
        Some(p)
    } else {
        None
    }
}

/// Join `parts` under [`root`].
#[must_use]
pub fn path<I, P>(parts: I) -> PathBuf
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut out = root();
    for part in parts {
        out.push(part);
    }
    out
}
