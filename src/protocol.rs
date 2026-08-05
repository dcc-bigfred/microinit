//! IPC protocol messages (length-prefixed JSON frames).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Runtime state of a service as reported over IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ServiceState {
    Pending,
    Starting,
    Running,
    Succeeded,
    Failed,
    Stopping,
    Stopped,
    Restarting,
    Disabled,
    /// Start was requested; blocked until `dependsOn` services are ready.
    /// Cleared by a successful start, or by manual `stop` / disable.
    WaitingForDependency,
}

impl std::fmt::Display for ServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Restarting => "restarting",
            Self::Disabled => "disabled",
            Self::WaitingForDependency => "waiting_for_dependency",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub state: ServiceState,
    pub pid: Option<i32>,
    pub restarts: u32,
    /// How many times `livenessProbe` failed (cumulative since boot / service add).
    #[serde(default)]
    pub liveness_failures: u32,
    pub enabled: bool,
    /// Service labels from config (stable key order).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// Kind of lifecycle event retained for `describe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ServiceEventKind {
    /// Every `set_state` transition (`from` → `to`).
    StateChange,
    Restart,
    LivenessFailed,
}

impl std::fmt::Display for ServiceEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::StateChange => "state_change",
            Self::Restart => "restart",
            Self::LivenessFailed => "liveness_failed",
        })
    }
}

/// One retained lifecycle event (ring-buffered per service).
///
/// Field presence by `kind`:
/// - `state_change`: `from` and `to` are set; `detail` is omitted
/// - `restart`: only `ts` + `kind`
/// - `liveness_failed`: optional `detail` (probe failure reason)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEvent {
    /// RFC3339 timestamp with millis.
    pub ts: String,
    pub kind: ServiceEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<ServiceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<ServiceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Service name + live state for dependency listings / graphs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepNode {
    pub name: String,
    pub state: ServiceState,
}

/// Actual identity of a running process (from `/proc/<pid>/status`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunningIdentity {
    pub uid: u32,
    pub gid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// Raw service object as it appears in its source config / drop-in file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSource {
    /// File the service was read from (drop-in path or main config path).
    pub path: String,
    /// Raw service object from that file (unmerged).
    pub json: serde_json::Value,
}

/// Output mode for `describe` (wire + CLI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DescribeOutput {
    #[default]
    Human,
    Json,
}

/// Full `describe` payload for one service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDescribe {
    pub status: ServiceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    /// Direct `dependsOn` (who this service needs).
    pub depends_on: Vec<DepNode>,
    /// Direct reverse deps (who lists this service in `dependsOn`).
    pub dependents: Vec<DepNode>,
    /// All nodes in the transitive dependency subgraph (with states).
    pub dep_nodes: Vec<DepNode>,
    /// Edges in the subgraph: `(from, to)` means `to` depends on `from`.
    pub dep_edges: Vec<(String, String)>,
    /// Oldest → newest, last [`crate::constants::EVENT_RETURN`] events.
    pub events: Vec<ServiceEvent>,
    /// Live process identity from `/proc/<pid>/status` when running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_as: Option<RunningIdentity>,
    /// Configured security context (definition). Absent on Android builds.
    #[cfg(not(target_os = "android"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_context: Option<crate::config::SecurityContext>,
    /// Raw source-file object; populated when `Request::Describe.output` is `json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ServiceSource>,
}

/// Log stream / severity for a captured line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LogLevel {
    Stdout,
    Stderr,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    /// RFC3339 timestamp
    pub ts: String,
    pub service: String,
    pub level: LogLevel,
    pub msg: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShutdownMode {
    Reboot,
    Poweroff,
    Halt,
}

impl std::fmt::Display for ShutdownMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Reboot => "reboot",
            Self::Poweroff => "poweroff",
            Self::Halt => "halt",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Request {
    List,
    Start {
        name: String,
        /// Skip `dependsOn` checks and start immediately.
        #[serde(default)]
        force: bool,
    },
    Stop {
        name: String,
    },
    Restart {
        name: String,
    },
    Status {
        name: String,
    },
    /// Rich status: deps, reverse deps, subgraph, recent lifecycle events.
    Describe {
        name: String,
        /// When `json`, the response includes the raw source-file service object.
        #[serde(default)]
        output: DescribeOutput,
    },
    Enable {
        name: String,
        enabled: bool,
    },
    Logs {
        name: Option<String>,
        follow: bool,
        lines: Option<usize>,
    },
    Shutdown {
        mode: ShutdownMode,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Response {
    Ok {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Error {
        message: String,
        /// Stable machine-readable code (e.g. "not_found", "disabled").
        /// Absent for errors without a canonical code; clients fall back to
        /// substring-matching `message` for backward compatibility.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    List {
        services: Vec<ServiceStatus>,
    },
    Status {
        status: ServiceStatus,
    },
    Describe {
        describe: Box<ServiceDescribe>,
    },
    /// One log line in a stream; ends with `Ok` when follow=false and buffer drained.
    Log {
        line: LogLine,
    },
    /// Keepalive on follow log streams so clients with idle read deadlines
    /// do not disconnect a quiet but healthy service.
    Heartbeat,
}
