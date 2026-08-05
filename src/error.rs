//! Error types for microinit.

use std::path::{Path, PathBuf};

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// I/O failure with the path that was being read or written.
    #[error("I/O error on {path}: {source}")]
    IoPath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

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

impl Error {
    /// Wrap an [`std::io::Error`] with the filesystem path involved.
    #[must_use]
    pub fn io_at(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::IoPath {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    /// Stable machine-readable error code for the IPC `Response::Error`.
    /// Clients map on this instead of substring-matching the human message.
    /// `None` means "no stable code"; clients fall back to the message.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Error::UnknownService(_) => Some("not_found"),
            Error::Disabled(_) => Some("disabled"),
            Error::Cycle(_) => Some("cycle"),
            _ => None,
        }
    }
}
