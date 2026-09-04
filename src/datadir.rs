//! Persistent data root resolution.
//!
//! Priority: `DATA_DIR` (absolute only), then `/data` (hub default).
//! Relative values are ignored so misconfiguration cannot silently redirect
//! data under the process working directory.

pub use dcc_daemon::datadir::{path, root, DEFAULT_ROOT, ENV_DATA_DIR};
