#![deny(unsafe_code)]

//! microinit library — init system and service supervisor for BigFred OS.
//!
//! Memory profile: **allocation-conscious** (see `CODING-GUIDELINES.md`). PID 1
//! paths prefer bounded buffers, poison-safe locks, and typed errors over panics.
//!
//! The `microinit` binary is a thin CLI over this crate. Integration tests live
//! under `tests/`.
//!
//! Signal installation in [`signals`] is the only intentional `unsafe` (documented
//! `SAFETY:` — async-signal-safe handlers only).
//!
//! Feature `init` (default) enables PID-1 early-boot, getty, unmount, and
//! `reboot(2)`. Android / supervise-only builds use `--no-default-features`.

pub mod cli;
pub mod config;
pub mod config_watch;
pub mod console;
pub mod constants;
pub mod datadir;
#[cfg(feature = "init")]
pub mod early_boot;
pub mod error;
pub mod graph;
pub mod init;
pub mod ipc;
pub mod labels;
pub mod liveness;
pub mod logs;
#[cfg(feature = "otel")]
pub mod otel;
pub mod otelenv;
pub mod protocol;
pub mod reaper;
#[cfg(not(target_os = "android"))]
pub mod security;
pub mod service;
#[cfg(feature = "init")]
#[cfg_attr(target_os = "android", allow(unsafe_code))]
pub mod shutdown;
#[allow(unsafe_code)]
pub mod signals;
pub mod supervisor;
pub mod syncutil;
#[cfg(feature = "init")]
pub mod unmount;
pub mod version;

pub use error::{Error, Result};
