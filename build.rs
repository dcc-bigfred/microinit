//! Emit build-time version env for `src/version.rs`.
//!
//! Prefer CI-provided `MICROINIT_GIT_COMMIT` / `MICROINIT_BUILD_TIME`.
//! Fall back to `git rev-parse` and UTC timestamp when unset.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");

    let commit = std::env::var("MICROINIT_GIT_COMMIT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(git_commit)
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MICROINIT_GIT_COMMIT={commit}");
    println!("cargo:rerun-if-env-changed=MICROINIT_GIT_COMMIT");

    let build_time = std::env::var("MICROINIT_BUILD_TIME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(utc_now);
    println!("cargo:rustc-env=MICROINIT_BUILD_TIME={build_time}");
    println!("cargo:rerun-if-env-changed=MICROINIT_BUILD_TIME");
}

fn git_commit() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn utc_now() -> String {
    // Prefer `date` for no chrono dep in build.rs; fall back to empty.
    Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}
