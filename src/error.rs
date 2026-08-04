//! Error types for microinit.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("config: {0}")]
    Config(String),

    #[error("service '{0}': {1}")]
    Service(String, String),

    #[error("dependency cycle involving '{0}'")]
    Cycle(String),

    #[error("unknown service '{0}'")]
    UnknownService(String),

    #[error("service '{0}' is disabled")]
    Disabled(String),

    #[error("early-boot failed with exit code {0}")]
    EarlyBoot(i32),

    #[error("unmount script failed with exit code {0}")]
    Unmount(i32),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("nix error: {0}")]
    Nix(#[from] nix::Error),

    #[error("{0}")]
    Other(String),
}
