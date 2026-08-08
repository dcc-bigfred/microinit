//! Configuration model and load/save for microinit.json + enabled override.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Hub-default control socket when `DATA_DIR` is unset (`/data/run/...`).
/// Prefer [`default_socket_path`] which honors `DATA_DIR`.
pub const DEFAULT_SOCKET: &str = "/data/run/microinit.sock";
pub const DEFAULT_CONSOLE: &str = "/dev/tty1";
pub const DEFAULT_LOGS_TTY: &str = "/dev/tty2";
pub const DEFAULT_INIT_LOGS_TTY: &str = "/dev/tty3";
pub const DEFAULT_LOG_LINES: usize = 300;
pub const DEFAULT_EARLY_BOOT: &str = "/etc/microinit/early-boot.sh";
pub const DEFAULT_UNMOUNT: &str = "/etc/microinit/unmount.sh";

/// Hub-default config path (`/data/etc/...` when data root is unset).
/// Prefer [`default_config_path`] which honors `DATA_DIR`.
pub const DEFAULT_CONFIG_PATH: &str = "/data/etc/microinit.json";

#[must_use]
pub fn default_config_path() -> PathBuf {
    crate::datadir::path(["etc", "microinit.json"])
}

/// Control socket under the persistent data root (`$DATA_DIR/run/microinit.sock`).
#[must_use]
pub fn default_socket_path() -> PathBuf {
    crate::datadir::path(["run", "microinit.sock"])
}

#[must_use]
pub fn default_example_path() -> PathBuf {
    crate::datadir::path(["etc", "microinit.json.example"])
}

#[must_use]
pub fn default_override_path() -> PathBuf {
    crate::datadir::path(["etc", "microinit.services.enabled-override.json"])
}

#[must_use]
pub fn default_dropins_dir() -> PathBuf {
    crate::datadir::path(["etc", "microinit.d", "services"])
}

#[must_use]
pub fn default_early_boot_override_path() -> PathBuf {
    crate::datadir::path(["etc", "microinit", "early-boot.sh"])
}

pub fn default_unmount_override_path() -> PathBuf {
    crate::datadir::path(["etc", "microinit", "unmount.sh"])
}

#[must_use]
pub fn default_logs_dir() -> PathBuf {
    crate::datadir::path(["logs"])
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogsConfig {
    #[serde(default = "default_logs_tty")]
    pub tty: String,
    /// TTY for microinit's own operational logs (start/stop/restart, errors).
    #[serde(default = "default_init_logs_tty")]
    pub init_tty: String,
    #[serde(default = "default_log_lines")]
    pub lines: usize,
    /// Directory for per-service `.log` files when [`Self::log_to_files`] is true.
    #[serde(default = "default_logs_dir_opt")]
    pub dir: Option<String>,
    /// Persist ring-buffer lines to `dir/<service>.log`. Default false (RAM + TTY only),
    /// suitable for embedded systems without a writable filesystem.
    #[serde(default)]
    pub log_to_files: bool,
}

fn default_logs_tty() -> String {
    DEFAULT_LOGS_TTY.to_string()
}

fn default_init_logs_tty() -> String {
    DEFAULT_INIT_LOGS_TTY.to_string()
}

fn default_log_lines() -> usize {
    DEFAULT_LOG_LINES
}

fn default_logs_dir_opt() -> Option<String> {
    Some(default_logs_dir().display().to_string())
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            tty: default_logs_tty(),
            init_tty: default_init_logs_tty(),
            lines: default_log_lines(),
            dir: default_logs_dir_opt(),
            log_to_files: false,
        }
    }
}

impl LogsConfig {
    /// Directory used for file sinks, or `None` when file logging is disabled / unset.
    #[must_use]
    pub fn effective_log_dir(&self) -> Option<std::path::PathBuf> {
        if !self.log_to_files {
            return None;
        }
        self.dir.as_ref().map(std::path::PathBuf::from)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LivenessProbe {
    /// Shell command; exit code must be in `success_exit_codes`. Mutually exclusive
    /// with `http_url` / `tcp_addr`.
    #[serde(default)]
    pub cmd: Option<String>,
    /// HTTP(S) URL to request. Mutually exclusive with `cmd` / `tcp_addr`.
    #[serde(default)]
    pub http_url: Option<String>,
    /// `host:port` TCP connect check. Mutually exclusive with `cmd` / `http_url`.
    #[serde(default)]
    pub tcp_addr: Option<String>,
    /// Accepted exit codes for `cmd` probes. Default `[0]`.
    #[serde(default = "default_success_codes")]
    pub success_exit_codes: Vec<i32>,
    /// Accepted HTTP status codes for `httpUrl` probes. Default `[200]`.
    #[serde(default = "default_http_accepted_codes")]
    pub http_accepted_codes: Vec<u16>,
    /// HTTP method for `httpUrl` probes. Default `GET`.
    #[serde(default = "default_http_method")]
    pub http_method: String,
    /// Seconds between probes. Default 60.
    #[serde(default = "default_liveness_interval")]
    pub interval: u64,
    /// Seconds before a probe attempt is aborted. Default 5.
    #[serde(default = "default_liveness_timeout")]
    pub timeout: u64,
}

fn default_liveness_interval() -> u64 {
    60
}

fn default_liveness_timeout() -> u64 {
    5
}

fn default_http_accepted_codes() -> Vec<u16> {
    vec![200]
}

fn default_http_method() -> String {
    "GET".into()
}

impl LivenessProbe {
    #[must_use]
    pub fn is_success(&self, code: i32) -> bool {
        self.success_exit_codes.contains(&code)
    }

    fn non_empty(opt: &Option<String>) -> bool {
        opt.as_deref().is_some_and(|s| !s.trim().is_empty())
    }

    /// How many of cmd / httpUrl / tcpAddr are set (non-empty).
    #[must_use]
    pub fn kind_count(&self) -> u32 {
        u32::from(Self::non_empty(&self.cmd))
            + u32::from(Self::non_empty(&self.http_url))
            + u32::from(Self::non_empty(&self.tcp_addr))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum RestartPolicy {
    /// Restart on every exit, including success (exit 0 / successExitCodes).
    Always,
    /// Restart only on non-success exits (default).
    #[default]
    OnError,
    /// Never auto-restart.
    None,
}

impl RestartPolicy {
    /// Whether an exit with the given success classification should trigger a restart.
    #[must_use]
    pub fn should_restart(self, success: bool) -> bool {
        match self {
            Self::Always => true,
            Self::OnError => !success,
            Self::None => false,
        }
    }

    /// Policies other than [`Self::None`] require `daemon=true`.
    #[must_use]
    pub fn requires_daemon(self) -> bool {
        !matches!(self, Self::None)
    }
}

fn default_restart_policy() -> RestartPolicy {
    RestartPolicy::OnError
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub daemon: bool,
    /// Auto-restart policy. Replaces the former `restart` bool
    /// (`true` → `onError`, `false` → `none`).
    #[serde(default = "default_restart_policy")]
    pub restart_policy: RestartPolicy,
    #[serde(default = "default_backoff")]
    pub restart_backoff: u64,
    #[serde(default = "default_success_codes")]
    pub success_exit_codes: Vec<i32>,
    /// After starting a daemon, wait this many seconds before deciding Running/Failed.
    /// Allows catching early crashes. Default 0 (only a short SysV daemonize grace).
    #[serde(default)]
    pub start_wait_secs: u64,
    /// After stop signal / stopCmd, wait this many seconds before SIGKILL. Default 5.
    #[serde(default = "default_shutdown_wait")]
    pub shutdown_wait_secs: u64,
    #[serde(default)]
    pub background: bool,
    /// Among currently ready services (`dependsOn` satisfied), lower values
    /// start earlier. Equal values fall back to alphabetical name. Default 100.
    #[serde(default = "default_order_priority")]
    pub order_priority: u64,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub cmd: Option<String>,
    #[serde(default)]
    pub start_cmd: Option<String>,
    #[serde(default)]
    pub stop_cmd: Option<String>,
    #[serde(default)]
    pub restart_cmd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_cwd")]
    pub cwd: String,
    /// Optional periodic health check; on failure the service is restarted.
    #[serde(default)]
    pub liveness_probe: Option<LivenessProbe>,
    /// Arbitrary key=value labels (e.g. `created-by=bigfred`). Stable order via BTreeMap.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Optional privilege drop / capabilities.
    ///
    /// Parsed on all platforms. On Android, a non-empty value fails
    /// [`Config::validate`]. On Linux, [`Config::prepare_security`] resolves it
    /// into [`Self::resolved_security`].
    #[serde(default)]
    pub security_context: Option<SecurityContext>,
    /// Cached resolution of [`Self::security_context`] (Linux only; not serialized).
    #[cfg(not(target_os = "android"))]
    #[serde(skip)]
    pub resolved_security: Option<crate::security::ResolvedIdentity>,
}

/// Per-service privilege drop and Linux capabilities.
///
/// On Android builds a configured context is rejected at validate time (not
/// silently ignored). On Linux it is applied at spawn via `security`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecurityContext {
    /// Login name or numeric uid.
    #[serde(default)]
    pub run_as_user: Option<String>,
    /// Group name or numeric gid; defaults to the user's primary gid when omitted.
    /// Required when `runAsUser` is a numeric uid with no passwd entry.
    #[serde(default)]
    pub run_as_group: Option<String>,
    /// Linux capability names (`CAP_` prefix optional). See `capabilities(7)`.
    /// When set, the list is **exclusive** (not additive to the parent's caps).
    #[serde(default)]
    pub capabilities: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_backoff() -> u64 {
    2
}

fn default_success_codes() -> Vec<i32> {
    vec![0]
}

fn default_shutdown_wait() -> u64 {
    5
}

fn default_cwd() -> String {
    "/".to_string()
}

fn default_order_priority() -> u64 {
    100
}

const LABEL_KEY_MAX: usize = 63;
const LABEL_VALUE_MAX: usize = 253;

/// Validate service label map (keys/values non-empty, key charset, length limits).
pub fn validate_labels(service: &str, labels: &BTreeMap<String, String>) -> Result<()> {
    for (key, value) in labels {
        if key.is_empty() {
            return Err(Error::Config(format!(
                "service '{service}': label key must not be empty"
            )));
        }
        if key.len() > LABEL_KEY_MAX {
            return Err(Error::Config(format!(
                "service '{service}': label key '{key}' exceeds {LABEL_KEY_MAX} characters"
            )));
        }
        if !is_valid_label_key(key) {
            return Err(Error::Config(format!(
                "service '{service}': invalid label key '{key}' (want [A-Za-z0-9][A-Za-z0-9._-]*)"
            )));
        }
        if value.is_empty() {
            return Err(Error::Config(format!(
                "service '{service}': label '{key}' value must not be empty"
            )));
        }
        if value.len() > LABEL_VALUE_MAX {
            return Err(Error::Config(format!(
                "service '{service}': label '{key}' value exceeds {LABEL_VALUE_MAX} characters"
            )));
        }
    }
    Ok(())
}

fn is_valid_label_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

impl ServiceConfig {
    /// Resolve start command: startCmd or `cmd start`.
    pub fn resolve_start(&self) -> Result<String> {
        if let Some(c) = &self.start_cmd {
            return Ok(c.clone());
        }
        self.cmd
            .as_ref()
            .map(|c| format!("{c} start"))
            .ok_or_else(|| Error::Service(self.name.clone(), "no startCmd or cmd defined".into()))
    }

    pub fn resolve_stop(&self) -> Result<String> {
        if let Some(c) = &self.stop_cmd {
            return Ok(c.clone());
        }
        self.cmd
            .as_ref()
            .map(|c| format!("{c} stop"))
            .ok_or_else(|| Error::Service(self.name.clone(), "no stopCmd or cmd defined".into()))
    }

    pub fn resolve_restart(&self) -> Result<String> {
        if let Some(c) = &self.restart_cmd {
            return Ok(c.clone());
        }
        if let Some(c) = &self.cmd {
            return Ok(format!("{c} restart"));
        }
        // Fall back to stop then start
        Ok(format!(
            "{} && {}",
            self.resolve_stop()?,
            self.resolve_start()?
        ))
    }

    pub fn is_success(&self, code: i32) -> bool {
        self.success_exit_codes.contains(&code)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenTelemetryConfig {
    #[serde(default)]
    pub enable: bool,
    #[serde(default = "default_otel_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_otel_protocol")]
    pub protocol: String,
    #[serde(default = "default_otel_service_name")]
    pub service_name: String,
    #[serde(default = "default_otel_export_interval")]
    pub export_interval_secs: u64,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

fn default_otel_endpoint() -> String {
    "http://127.0.0.1:4318".into()
}

fn default_otel_protocol() -> String {
    "http".into()
}

fn default_otel_service_name() -> String {
    "microinit".into()
}

fn default_otel_export_interval() -> u64 {
    15
}

impl Default for OpenTelemetryConfig {
    fn default() -> Self {
        Self {
            enable: false,
            endpoint: default_otel_endpoint(),
            protocol: default_otel_protocol(),
            service_name: default_otel_service_name(),
            export_interval_secs: default_otel_export_interval(),
            headers: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub logs: LogsConfig,
    #[serde(default = "default_socket")]
    pub socket: String,
    #[serde(default = "default_console")]
    pub console: String,
    /// Extra Unix-socket peer uids allowed besides the daemon's own uid.
    /// Login names resolved against `/etc/passwd` at load time (fail-closed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub socket_allow_users: Vec<String>,
    #[serde(default)]
    pub open_telemetry: OpenTelemetryConfig,
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
}

fn default_version() -> u32 {
    1
}

fn default_socket() -> String {
    default_socket_path().display().to_string()
}

fn default_console() -> String {
    DEFAULT_CONSOLE.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            logs: LogsConfig::default(),
            socket: default_socket(),
            console: default_console(),
            socket_allow_users: Vec::new(),
            open_telemetry: OpenTelemetryConfig::default(),
            services: Vec::new(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        let mut names = std::collections::HashSet::new();
        for svc in &self.services {
            if svc.name.is_empty() {
                return Err(Error::Config("service with empty name".into()));
            }
            if !names.insert(svc.name.clone()) {
                return Err(Error::Config(format!(
                    "duplicate service name '{}'",
                    svc.name
                )));
            }
            if matches!(svc.restart_policy, RestartPolicy::Always) && !svc.daemon {
                return Err(Error::Config(format!(
                    "service '{}': restartPolicy=always requires daemon=true",
                    svc.name
                )));
            }
            if svc.start_cmd.is_none() && svc.cmd.is_none() {
                return Err(Error::Config(format!(
                    "service '{}': need startCmd or cmd",
                    svc.name
                )));
            }
            if let Some(ref probe) = svc.liveness_probe {
                if probe.kind_count() != 1 {
                    return Err(Error::Config(format!(
                        "service '{}': livenessProbe needs exactly one of cmd, httpUrl, tcpAddr",
                        svc.name
                    )));
                }
                if LivenessProbe::non_empty(&probe.cmd) && probe.success_exit_codes.is_empty() {
                    return Err(Error::Config(format!(
                        "service '{}': livenessProbe.successExitCodes must not be empty",
                        svc.name
                    )));
                }
                if LivenessProbe::non_empty(&probe.http_url) {
                    if probe.http_accepted_codes.is_empty() {
                        return Err(Error::Config(format!(
                            "service '{}': livenessProbe.httpAcceptedCodes must not be empty",
                            svc.name
                        )));
                    }
                    if probe.http_method.trim().is_empty() {
                        return Err(Error::Config(format!(
                            "service '{}': livenessProbe.httpMethod must not be empty",
                            svc.name
                        )));
                    }
                }
                if probe.interval == 0 {
                    return Err(Error::Config(format!(
                        "service '{}': livenessProbe.interval must be >= 1",
                        svc.name
                    )));
                }
                if probe.timeout == 0 {
                    return Err(Error::Config(format!(
                        "service '{}': livenessProbe.timeout must be >= 1",
                        svc.name
                    )));
                }
            }
            validate_labels(&svc.name, &svc.labels)?;
            if let Some(ref sec) = svc.security_context {
                #[cfg(target_os = "android")]
                {
                    let _ = sec;
                    return Err(Error::Config(format!(
                        "service '{}': securityContext is not supported on Android",
                        svc.name
                    )));
                }
                #[cfg(not(target_os = "android"))]
                {
                    if let Some(ref u) = sec.run_as_user {
                        if u.trim().is_empty() {
                            return Err(Error::Config(format!(
                                "service '{}': securityContext.runAsUser must not be empty",
                                svc.name
                            )));
                        }
                    }
                    if let Some(ref g) = sec.run_as_group {
                        if g.trim().is_empty() {
                            return Err(Error::Config(format!(
                                "service '{}': securityContext.runAsGroup must not be empty",
                                svc.name
                            )));
                        }
                    }
                    for cap in &sec.capabilities {
                        crate::security::validate_cap_name(cap).map_err(|e| {
                            Error::Config(format!("service '{}': securityContext.{}", svc.name, e))
                        })?;
                    }
                }
            }
        }
        for svc in &self.services {
            for dep in &svc.depends_on {
                if !names.contains(dep) {
                    return Err(Error::Config(format!(
                        "service '{}': dependsOn unknown service '{}'",
                        svc.name, dep
                    )));
                }
            }
        }
        // Fail-closed: unknown names in socketAllowUsers abort config load.
        let _ = self.resolved_ipc_allow()?;
        Ok(())
    }

    /// Resolve [`Self::socket_allow_users`] into IPC peer allowlist + socket group.
    ///
    /// Empty allowlist → only the daemon uid may connect; socket stays `0600`.
    /// Non-empty → those uids (plus daemon uid) may connect; socket is `0660`
    /// owned by `daemon_uid:<socket_gid>`.
    ///
    /// **`socket_gid` is taken from the first entry** in `socketAllowUsers`:
    /// prefer a group whose name matches the login (e.g. `bigfred:bigfred`),
    /// else the user's primary gid from passwd. Later entries are allowed by
    /// uid check only — they must share that group (or be root) to open a
    /// `0660` socket. Put the intended socket group owner first.
    pub fn resolved_ipc_allow(&self) -> Result<crate::ipc::IpcAllow> {
        use nix::unistd::{Group, Uid, User};
        let mut allow_uids = Vec::new();
        let mut socket_gid: Option<u32> = None;
        for raw in &self.socket_allow_users {
            let name = raw.trim();
            if name.is_empty() {
                return Err(Error::Config(
                    "socketAllowUsers entry must not be empty".into(),
                ));
            }
            let u = User::from_name(name)
                .map_err(|e| Error::Config(format!("socketAllowUsers lookup '{name}': {e}")))?
                .ok_or_else(|| Error::Config(format!("socketAllowUsers: unknown user '{name}'")))?;
            allow_uids.push(u.uid.as_raw());
            if socket_gid.is_none() {
                // Prefer the user's primary group name matching the login when
                // present (e.g. bigfred:bigfred); otherwise use passwd gid.
                let gid = Group::from_name(name)
                    .ok()
                    .flatten()
                    .map(|g| g.gid.as_raw())
                    .unwrap_or_else(|| u.gid.as_raw());
                socket_gid = Some(gid);
            }
        }
        Ok(crate::ipc::IpcAllow {
            daemon_uid: Uid::current().as_raw(),
            allow_uids,
            socket_gid,
        })
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut ServiceConfig> {
        self.services.iter_mut().find(|s| s.name == name)
    }

    pub fn get(&self, name: &str) -> Option<&ServiceConfig> {
        self.services.iter().find(|s| s.name == name)
    }

    /// Resolve each service's `securityContext` into a cached identity (Linux).
    ///
    /// Call after [`Self::validate`]. Failures (unknown user, etc.) surface here
    /// so spawn/liveness never hit NSS on the hot path.
    #[cfg(not(target_os = "android"))]
    pub fn prepare_security(&mut self) -> Result<()> {
        for svc in &mut self.services {
            svc.resolved_security = match &svc.security_context {
                Some(ctx) => crate::security::resolve(ctx)?,
                None => None,
            };
        }
        Ok(())
    }
}

/// Merge enabled overrides onto config (in place).
pub fn apply_enabled_override(cfg: &mut Config, override_map: &HashMap<String, bool>) {
    for svc in &mut cfg.services {
        if let Some(&enabled) = override_map.get(&svc.name) {
            svc.enabled = enabled;
        }
    }
}

pub fn load_override(path: &Path) -> Result<HashMap<String, bool>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let data = fs::read_to_string(path)?;
    let map: HashMap<String, bool> = serde_json::from_str(&data)?;
    Ok(map)
}

pub fn save_override(path: &Path, map: &HashMap<String, bool>) -> Result<()> {
    write_json_atomic(path, map)
}

pub fn load_config(path: &Path) -> Result<Config> {
    let data = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    let mut cfg: Config = serde_json::from_str(&data)?;
    cfg.validate()?;
    #[cfg(not(target_os = "android"))]
    cfg.prepare_security()?;
    Ok(cfg)
}

pub fn save_config(path: &Path, cfg: &Config) -> Result<()> {
    write_json_atomic(path, cfg)
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
        }
    }
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, format!("{data}\n")).map_err(|e| Error::io_at(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| Error::io_at(path, e))?;
    Ok(())
}

/// Drop-in file: `{ "services": [ ... ] }` under `microinit.d/services/**/*.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DropinFile {
    #[serde(default)]
    services: Vec<ServiceConfig>,
}

/// Collect relative paths of `*.json` under `root`, sorted lexicographically.
pub(crate) fn collect_dropin_rel_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk(&path, root, out)?;
            } else if ft.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
        }
        Ok(())
    }
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// Merge drop-in service definitions into `cfg`.
///
/// Files under `dropins_root` are processed in lexicographic order of their
/// relative path; for a given service `name`, later files win.
pub fn merge_service_dropins(cfg: &mut Config, dropins_root: &Path) -> Result<()> {
    let rels = collect_dropin_rel_paths(dropins_root)?;
    for rel in rels {
        let path = dropins_root.join(&rel);
        let data = fs::read_to_string(&path)?;
        let dropin: DropinFile = serde_json::from_str(&data)
            .map_err(|e| Error::Config(format!("drop-in {}: {e}", path.display())))?;
        for svc in dropin.services {
            if let Some(existing) = cfg.services.iter_mut().find(|s| s.name == svc.name) {
                *existing = svc;
            } else {
                cfg.services.push(svc);
            }
        }
    }
    Ok(())
}

/// Ensure config directory + default config + example exist; load, merge drop-ins, apply override.
pub fn load_or_create(
    config_path: &Path,
    example_path: &Path,
    override_path: &Path,
) -> Result<Config> {
    load_or_create_with_dropins(
        config_path,
        example_path,
        override_path,
        &default_dropins_dir(),
    )
}

/// Like [`load_or_create`] with an explicit drop-ins directory (tests / custom layouts).
pub fn load_or_create_with_dropins(
    config_path: &Path,
    example_path: &Path,
    override_path: &Path,
    dropins_root: &Path,
) -> Result<Config> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if !example_path.exists() {
        save_config(example_path, &example_config())?;
    }

    if !config_path.exists() {
        save_config(config_path, &Config::default())?;
    }

    let data = fs::read_to_string(config_path)?;
    let mut cfg: Config = serde_json::from_str(&data)?;
    merge_service_dropins(&mut cfg, dropins_root)?;
    let ov = load_override(override_path)?;
    apply_enabled_override(&mut cfg, &ov);
    cfg.validate()?;
    #[cfg(not(target_os = "android"))]
    cfg.prepare_security()?;
    Ok(cfg)
}

pub fn example_config() -> Config {
    Config {
        version: 1,
        logs: LogsConfig {
            tty: DEFAULT_LOGS_TTY.to_string(),
            init_tty: DEFAULT_INIT_LOGS_TTY.to_string(),
            lines: DEFAULT_LOG_LINES,
            dir: Some(default_logs_dir().display().to_string()),
            log_to_files: false,
        },
        socket: DEFAULT_SOCKET.to_string(),
        console: DEFAULT_CONSOLE.to_string(),
        socket_allow_users: Vec::new(),
        open_telemetry: OpenTelemetryConfig::default(),
        services: vec![
            ServiceConfig {
                name: "network".into(),
                enabled: true,
                daemon: false,
                restart_policy: RestartPolicy::None,
                restart_backoff: 2,
                success_exit_codes: vec![0],
                start_wait_secs: 0,
                shutdown_wait_secs: 5,
                background: false,
                order_priority: 30,
                depends_on: vec![],
                cmd: Some("/etc/init.d/network".into()),
                start_cmd: None,
                stop_cmd: None,
                restart_cmd: None,
                env: HashMap::new(),
                cwd: "/".into(),
                liveness_probe: Some(LivenessProbe {
                    cmd: Some("/usr/sbin/configure-ethernet check".into()),
                    http_url: None,
                    tcp_addr: None,
                    success_exit_codes: vec![0],
                    http_accepted_codes: vec![200],
                    http_method: "GET".into(),
                    interval: 30,
                    timeout: 5,
                }),
                labels: BTreeMap::new(),
                security_context: None,
                #[cfg(not(target_os = "android"))]
                resolved_security: None,
            },
            ServiceConfig {
                name: "redis".into(),
                enabled: true,
                daemon: true,
                restart_policy: RestartPolicy::OnError,
                restart_backoff: 2,
                success_exit_codes: vec![0],
                start_wait_secs: 0,
                shutdown_wait_secs: 5,
                background: false,
                order_priority: 100,
                depends_on: vec!["network".into()],
                cmd: Some("/etc/init.d/redis".into()),
                start_cmd: None,
                stop_cmd: None,
                restart_cmd: None,
                env: HashMap::new(),
                cwd: "/".into(),
                liveness_probe: None,
                labels: BTreeMap::new(),
                security_context: None,
                #[cfg(not(target_os = "android"))]
                resolved_security: None,
            },
            ServiceConfig {
                name: "remote-icmp".into(),
                enabled: true,
                daemon: true,
                restart_policy: RestartPolicy::OnError,
                restart_backoff: 5,
                success_exit_codes: vec![0],
                start_wait_secs: 0,
                shutdown_wait_secs: 5,
                background: true,
                order_priority: 210,
                depends_on: vec!["network".into()],
                cmd: None,
                start_cmd: Some(format!(
                    "/usr/bin/bigfred-remote-icmp --config {}/etc/loco-server.conf",
                    crate::datadir::root().display()
                )),
                stop_cmd: Some("killall bigfred-remote-icmp".into()),
                restart_cmd: None,
                env: HashMap::new(),
                cwd: "/".into(),
                liveness_probe: None,
                labels: BTreeMap::new(),
                security_context: Some(SecurityContext {
                    run_as_user: Some("nobody".into()),
                    run_as_group: None,
                    capabilities: vec!["CAP_NET_RAW".into()],
                }),
                #[cfg(not(target_os = "android"))]
                resolved_security: None,
            },
        ],
    }
}

/// Paths used at runtime (overridable for tests).
#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub example: PathBuf,
    pub override_file: PathBuf,
    pub dropins_dir: PathBuf,
    pub early_boot: PathBuf,
    pub early_boot_override: PathBuf,
    pub unmount: PathBuf,
    pub unmount_override: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            config: default_config_path(),
            example: default_example_path(),
            override_file: default_override_path(),
            dropins_dir: default_dropins_dir(),
            early_boot: PathBuf::from(DEFAULT_EARLY_BOOT),
            early_boot_override: default_early_boot_override_path(),
            unmount: PathBuf::from(DEFAULT_UNMOUNT),
            unmount_override: default_unmount_override_path(),
        }
    }
}
