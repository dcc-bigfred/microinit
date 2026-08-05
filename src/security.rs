//! Privilege drop and Linux capabilities for supervised services.
//!
//! Completely disabled on Android (`cfg(not(target_os = "android"))` at the
//! module boundary in `lib.rs`).

#![allow(unsafe_code)]

use nix::unistd::{self, Gid, Group, Uid, User};

use crate::config::SecurityContext;
use crate::error::{Error, Result};

/// Allowlisted capability names and their Linux capability numbers
/// (`linux/capability.h`). Single source of truth for validation and apply.
const CAP_TABLE: &[(&str, u8)] = &[
    ("CHOWN", 0),
    ("DAC_OVERRIDE", 1),
    ("DAC_READ_SEARCH", 2),
    ("FOWNER", 3),
    ("FSETID", 4),
    ("KILL", 5),
    ("SETGID", 6),
    ("SETUID", 7),
    ("SETPCAP", 8),
    ("LINUX_IMMUTABLE", 9),
    ("NET_BIND_SERVICE", 10),
    ("NET_BROADCAST", 11),
    ("NET_ADMIN", 12),
    ("NET_RAW", 13),
    ("IPC_LOCK", 14),
    ("IPC_OWNER", 15),
    ("SYS_MODULE", 16),
    ("SYS_RAWIO", 17),
    ("SYS_CHROOT", 18),
    ("SYS_PTRACE", 19),
    ("SYS_PACCT", 20),
    ("SYS_ADMIN", 21),
    ("SYS_BOOT", 22),
    ("SYS_NICE", 23),
    ("SYS_RESOURCE", 24),
    ("SYS_TIME", 25),
    ("SYS_TTY_CONFIG", 26),
    ("MKNOD", 27),
    ("LEASE", 28),
    ("AUDIT_WRITE", 29),
    ("AUDIT_CONTROL", 30),
    ("SETFCAP", 31),
    ("MAC_OVERRIDE", 32),
    ("MAC_ADMIN", 33),
    ("SYSLOG", 34),
    ("WAKE_ALARM", 35),
    ("BLOCK_SUSPEND", 36),
    ("AUDIT_READ", 37),
    ("PERFMON", 38),
    ("BPF", 39),
    ("CHECKPOINT_RESTORE", 40),
];

/// Highest known capability number in [`CAP_TABLE`] (inclusive).
const CAP_LAST: u8 = 40;

/// Capability names accepted in `securityContext.capabilities` (without requiring
/// the `CAP_` prefix).
pub const KNOWN_CAPABILITIES: &[&str] = {
    // Keep a parallel name list for docs/tests without allocating at runtime.
    &[
        "AUDIT_CONTROL",
        "AUDIT_READ",
        "AUDIT_WRITE",
        "BLOCK_SUSPEND",
        "BPF",
        "CHECKPOINT_RESTORE",
        "CHOWN",
        "DAC_OVERRIDE",
        "DAC_READ_SEARCH",
        "FOWNER",
        "FSETID",
        "IPC_LOCK",
        "IPC_OWNER",
        "KILL",
        "LEASE",
        "LINUX_IMMUTABLE",
        "MAC_ADMIN",
        "MAC_OVERRIDE",
        "MKNOD",
        "NET_ADMIN",
        "NET_BIND_SERVICE",
        "NET_BROADCAST",
        "NET_RAW",
        "PERFMON",
        "SETFCAP",
        "SETGID",
        "SETPCAP",
        "SETUID",
        "SYS_ADMIN",
        "SYS_BOOT",
        "SYS_CHROOT",
        "SYSLOG",
        "SYS_MODULE",
        "SYS_NICE",
        "SYS_PACCT",
        "SYS_PTRACE",
        "SYS_RAWIO",
        "SYS_RESOURCE",
        "SYS_TIME",
        "SYS_TTY_CONFIG",
        "WAKE_ALARM",
    ]
};

/// Normalize a capability name: strip optional `CAP_` prefix, uppercase.
#[must_use]
pub fn normalize_cap_name(raw: &str) -> String {
    let s = raw.trim();
    let s = s
        .strip_prefix("CAP_")
        .or_else(|| s.strip_prefix("cap_"))
        .unwrap_or(s);
    s.to_ascii_uppercase()
}

/// Validate a single capability name (config-time).
pub fn validate_cap_name(raw: &str) -> Result<()> {
    let n = normalize_cap_name(raw);
    if n.is_empty() {
        return Err(Error::Config("empty capability name".into()));
    }
    if !CAP_TABLE.iter().any(|(name, _)| *name == n) {
        return Err(Error::Config(format!("unknown capability '{raw}'")));
    }
    Ok(())
}

fn cap_number(normalized: &str) -> Result<u8> {
    CAP_TABLE
        .iter()
        .find(|(name, _)| *name == normalized)
        .map(|(_, n)| *n)
        .ok_or_else(|| Error::Security(format!("unknown capability '{normalized}'")))
}

/// Resolved identity ready for `pre_exec` application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIdentity {
    /// Target uid; `None` means leave uid unchanged.
    pub uid: Option<u32>,
    /// Target gid; `None` means leave gid unchanged.
    pub gid: Option<u32>,
    /// Capability numbers to keep as ambient/inheritable/permitted/effective.
    /// Empty means clear all capabilities after the identity change.
    pub caps: Vec<u8>,
    /// Suggested `HOME` from passwd (best-effort).
    pub home: Option<String>,
    /// Suggested `USER` / `LOGNAME` from passwd (best-effort).
    pub username: Option<String>,
}

impl ResolvedIdentity {
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.uid.is_none() && self.gid.is_none() && self.caps.is_empty()
    }

    #[must_use]
    pub fn drops_identity(&self) -> bool {
        self.uid.is_some() || self.gid.is_some()
    }
}

/// Resolve `SecurityContext` into a concrete identity.
///
/// Returns `Ok(None)` when the context is empty (no user/group/caps).
/// User/group lookup failures are spawn-time / prepare-time errors (`Error::Security`).
pub fn resolve(ctx: &SecurityContext) -> Result<Option<ResolvedIdentity>> {
    let has_user = ctx
        .run_as_user
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    let has_group = ctx
        .run_as_group
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    if !has_user && !has_group && ctx.capabilities.is_empty() {
        return Ok(None);
    }

    let mut uid: Option<u32> = None;
    let mut primary_gid: Option<u32> = None;
    let mut home: Option<String> = None;
    let mut username: Option<String> = None;

    if let Some(ref user_spec) = ctx.run_as_user {
        let spec = user_spec.trim();
        if spec.is_empty() {
            return Err(Error::Security("runAsUser is empty".into()));
        }
        if let Ok(n) = spec.parse::<u32>() {
            uid = Some(n);
            if let Ok(Some(u)) = User::from_uid(Uid::from_raw(n)) {
                primary_gid = Some(u.gid.as_raw());
                home = Some(u.dir.display().to_string());
                username = Some(u.name);
            } else if !has_group {
                return Err(Error::Security(format!(
                    "runAsUser '{spec}' has no passwd entry; set runAsGroup explicitly"
                )));
            }
        } else {
            let u = User::from_name(spec)
                .map_err(|e| Error::Security(format!("lookup user '{spec}': {e}")))?
                .ok_or_else(|| Error::Security(format!("unknown user '{spec}'")))?;
            uid = Some(u.uid.as_raw());
            primary_gid = Some(u.gid.as_raw());
            home = Some(u.dir.display().to_string());
            username = Some(u.name);
        }
    }

    let gid = if let Some(ref group_spec) = ctx.run_as_group {
        let spec = group_spec.trim();
        if spec.is_empty() {
            return Err(Error::Security("runAsGroup is empty".into()));
        }
        Some(if let Ok(n) = spec.parse::<u32>() {
            n
        } else {
            let g = Group::from_name(spec)
                .map_err(|e| Error::Security(format!("lookup group '{spec}': {e}")))?
                .ok_or_else(|| Error::Security(format!("unknown group '{spec}'")))?;
            g.gid.as_raw()
        })
    } else {
        primary_gid
    };

    let mut caps = Vec::with_capacity(ctx.capabilities.len());
    for raw in &ctx.capabilities {
        let n = normalize_cap_name(raw);
        caps.push(cap_number(&n)?);
    }
    caps.sort_unstable();
    caps.dedup();

    let ident = ResolvedIdentity {
        uid,
        gid,
        caps,
        home,
        username,
    };
    if ident.is_noop() {
        Ok(None)
    } else {
        Ok(Some(ident))
    }
}

/// Apply identity in the child after fork, before exec.
///
/// Order: keepcaps → bounding-set drop → initgroups (or setgroups([])) →
/// setgid → setuid → capset + ambient → `PR_SET_NO_NEW_PRIVS`.
///
/// When a passwd username is known, [`unistd::initgroups`] installs that
/// user's supplementary groups from `/etc/group` (e.g. `bigfred` ∈ `dialout`).
/// Numeric uids without a passwd entry keep the fail-closed `setgroups([])`.
///
/// # Safety
///
/// Must only be called from a `Command::pre_exec` closure (single-threaded
/// child between fork and exec). Success path avoids heap allocation after the
/// first syscall; error paths may allocate for diagnostics.
pub fn apply_pre_exec(ident: &ResolvedIdentity) -> Result<()> {
    if ident.is_noop() {
        return Ok(());
    }

    let want_caps = !ident.caps.is_empty();
    let drop_id = ident.drops_identity();

    // SAFETY: PR_SET_KEEPCAPS is a well-defined prctl; failure is reported.
    if want_caps {
        let rc = unsafe { libc::prctl(libc::PR_SET_KEEPCAPS, 1i64, 0, 0, 0) };
        if rc != 0 {
            return Err(Error::Security(format!(
                "PR_SET_KEEPCAPS: {}",
                std::io::Error::last_os_error()
            )));
        }
    }

    // Shrink the capability bounding set while still privileged.
    drop_bounding_set(&ident.caps)?;

    // Fail-closed vs inheriting the parent's (often root) supplementary groups.
    // Prefer initgroups(username) so /etc/group memberships (e.g. dialout) apply.
    // User namespaces with `/proc/self/setgroups=deny` are unsupported for
    // securityContext identity drops.
    if drop_id {
        apply_groups(ident)?;
    }

    if let Some(gid) = ident.gid {
        unistd::setgid(Gid::from_raw(gid))
            .map_err(|e| Error::Security(format!("setgid({gid}): {e}")))?;
    }

    if let Some(uid) = ident.uid {
        unistd::setuid(Uid::from_raw(uid))
            .map_err(|e| Error::Security(format!("setuid({uid}): {e}")))?;
    }

    // Always install an explicit capability set after identity change:
    // requested caps, or empty (clear everything) when dropping identity.
    if want_caps || drop_id {
        set_capabilities(&ident.caps)?;
    }

    // SAFETY: PR_SET_NO_NEW_PRIVS blocks future privilege gains via execve
    // (setuid binaries / file capabilities).
    let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1i64, 0, 0, 0) };
    if rc != 0 {
        return Err(Error::Security(format!(
            "PR_SET_NO_NEW_PRIVS: {}",
            std::io::Error::last_os_error()
        )));
    }

    Ok(())
}

fn apply_groups(ident: &ResolvedIdentity) -> Result<()> {
    use std::ffi::CString;
    if let (Some(ref name), Some(gid)) = (&ident.username, ident.gid) {
        let cname = CString::new(name.as_str()).map_err(|_| {
            Error::Security(format!("username '{name}' contains NUL"))
        })?;
        unistd::initgroups(&cname, Gid::from_raw(gid)).map_err(|e| {
            Error::Security(format!(
                "initgroups({name}, {gid}): {e} (required when runAsUser/runAsGroup is set)"
            ))
        })?;
        return Ok(());
    }
    unistd::setgroups(&[]).map_err(|e| {
        Error::Security(format!(
            "setgroups: {e} (required when runAsUser/runAsGroup is set)"
        ))
    })?;
    Ok(())
}

/// Install [`apply_pre_exec`] on `cmd` via `CommandExt::pre_exec`.
pub fn attach_pre_exec(cmd: &mut std::process::Command, ident: &ResolvedIdentity) {
    use std::os::unix::process::CommandExt;
    let owned = ident.clone();
    // SAFETY: the closure only runs in the single-threaded child between fork
    // and exec; see [`apply_pre_exec`].
    unsafe {
        cmd.pre_exec(move || {
            apply_pre_exec(&owned).map_err(|e| std::io::Error::other(e.to_string()))
        });
    }
}

fn drop_bounding_set(keep: &[u8]) -> Result<()> {
    for cap in 0..=CAP_LAST {
        if keep.contains(&cap) {
            continue;
        }
        // SAFETY: PR_CAPBSET_DROP removes `cap` from the bounding set.
        // Best-effort: some containers lack CAP_SETPCAP or reject drops with
        // EINVAL/EPERM; identity drop and capset below remain authoritative.
        let _ = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap as libc::c_ulong, 0, 0, 0) };
    }
    Ok(())
}

// Linux capability ABI (capability.h) — not always exported by the `libc` crate.
const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

#[repr(C)]
struct CapUserHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CapUserData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

unsafe extern "C" {
    fn capset(hdrp: *mut CapUserHeader, datap: *const CapUserData) -> libc::c_int;
}

fn set_capabilities(caps: &[u8]) -> Result<()> {
    let mut low: u32 = 0;
    let mut high: u32 = 0;
    for &c in caps {
        if c < 32 {
            low |= 1u32 << c;
        } else if c < 64 {
            high |= 1u32 << (c - 32);
        } else {
            return Err(Error::Security(format!(
                "capability number {c} out of range"
            )));
        }
    }

    // SAFETY: capset with version 3 header + two data words is the documented
    // Linux ABI for setting process capabilities.
    let mut header = CapUserHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let data = [
        CapUserData {
            effective: low,
            permitted: low,
            inheritable: low,
        },
        CapUserData {
            effective: high,
            permitted: high,
            inheritable: high,
        },
    ];
    let rc = unsafe { capset(&mut header, data.as_ptr()) };
    if rc != 0 {
        return Err(Error::Security(format!(
            "capset: {}",
            std::io::Error::last_os_error()
        )));
    }

    // SAFETY: PR_CAP_AMBIENT_* are documented prctl operations.
    let rc = unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,
            0,
            0,
            0,
        )
    };
    if rc != 0 {
        return Err(Error::Security(format!(
            "PR_CAP_AMBIENT_CLEAR_ALL: {}",
            std::io::Error::last_os_error()
        )));
    }

    for &c in caps {
        let rc = unsafe {
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_RAISE as libc::c_ulong,
                c as libc::c_ulong,
                0,
                0,
            )
        };
        if rc != 0 {
            return Err(Error::Security(format!(
                "PR_CAP_AMBIENT_RAISE({c}): {}",
                std::io::Error::last_os_error()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_prefix() {
        assert_eq!(normalize_cap_name("CAP_NET_RAW"), "NET_RAW");
        assert_eq!(normalize_cap_name("cap_net_raw"), "NET_RAW");
        assert_eq!(normalize_cap_name("NET_RAW"), "NET_RAW");
        assert_eq!(
            normalize_cap_name("  net_bind_service "),
            "NET_BIND_SERVICE"
        );
    }

    #[test]
    fn validate_known_and_unknown() {
        assert!(validate_cap_name("CAP_NET_BIND_SERVICE").is_ok());
        assert!(validate_cap_name("NET_RAW").is_ok());
        assert!(validate_cap_name("NOT_A_CAP").is_err());
        assert!(validate_cap_name("").is_err());
    }

    #[test]
    fn cap_table_covers_known_list() {
        for name in KNOWN_CAPABILITIES {
            assert!(
                CAP_TABLE.iter().any(|(n, _)| n == name),
                "{name} missing from CAP_TABLE"
            );
        }
        for (name, _) in CAP_TABLE {
            assert!(
                KNOWN_CAPABILITIES.contains(name),
                "{name} missing from KNOWN_CAPABILITIES"
            );
        }
    }

    #[test]
    fn resolve_empty_is_none() {
        let ctx = SecurityContext::default();
        assert!(resolve(&ctx).unwrap().is_none());
    }

    #[test]
    fn resolve_numeric_user() {
        let ctx = SecurityContext {
            run_as_user: Some("0".into()),
            run_as_group: Some("0".into()),
            capabilities: vec![],
        };
        let ident = resolve(&ctx).unwrap().unwrap();
        assert_eq!(ident.uid, Some(0));
        assert_eq!(ident.gid, Some(0));
    }

    #[test]
    fn resolve_numeric_uid_without_passwd_requires_group() {
        // Extremely unlikely to exist in passwd; still require explicit group.
        let ctx = SecurityContext {
            run_as_user: Some("4294967294".into()), // -2 as u32 often unused
            run_as_group: None,
            capabilities: vec![],
        };
        // May succeed if passwd has the entry; if not, must error about runAsGroup.
        match resolve(&ctx) {
            Ok(Some(ident)) => {
                assert!(
                    ident.gid.is_some(),
                    "passwd entry should supply primary gid"
                );
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("runAsGroup") || msg.contains("passwd"),
                    "{msg}"
                );
            }
            Ok(None) => panic!("expected identity or error"),
        }
    }
}
