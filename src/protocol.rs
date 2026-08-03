//! IPC protocol messages (length-prefixed JSON frames).

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
    pub enabled: bool,
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
    Ok,
    Error {
        message: String,
    },
    List {
        services: Vec<ServiceStatus>,
    },
    Status {
        status: ServiceStatus,
    },
    /// One log line in a stream; ends with `Ok` when follow=false and buffer drained.
    Log {
        line: LogLine,
    },
}
