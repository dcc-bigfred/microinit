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

pub mod cli;
pub mod config;
pub mod config_watch;
pub mod console;
pub mod constants;
pub mod datadir;
pub mod early_boot;
pub mod error;
pub mod graph;
pub mod init;
pub mod ipc;
pub mod liveness;
pub mod logs;
#[cfg(feature = "otel")]
pub mod otel;
pub mod protocol;
pub mod reaper;
pub mod service;
pub mod shutdown;
#[allow(unsafe_code)]
pub mod signals;
pub mod supervisor;
pub mod syncutil;

pub use error::{Error, Result};
