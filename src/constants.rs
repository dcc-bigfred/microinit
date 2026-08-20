//! Named timing / size constants (avoid magic numbers on PID 1 paths).

use std::time::Duration;

/// Foreground service settle timeout during boot.
pub const BOOT_FG_SETTLE: Duration = Duration::from_secs(120);
/// Background service settle timeout during boot.
pub const BOOT_BG_SETTLE: Duration = Duration::from_secs(60);
/// Dependency wait timeout before start.
pub const DEP_WAIT: Duration = Duration::from_secs(120);
/// How long to wait to see if a start script daemonizes and exits when
/// `startWaitSecs` is 0.
pub const DAEMONIZE_GRACE: Duration = Duration::from_millis(300);
/// Control-channel poll interval in monitor threads.
pub const CTL_POLL: Duration = Duration::from_millis(200);
/// Fallback grace for SIGTERM→SIGKILL when no service config is available
/// (e.g. job start timeout). Prefer `ServiceConfig::shutdown_wait_secs`.
pub const STOP_GRACE_SECS: u64 = 5;
/// Delay after stop-all before sending Quit.
pub const SHUTDOWN_STOP_WAIT: Duration = Duration::from_secs(2);
/// Delay after Quit before returning from stop_all.
pub const SHUTDOWN_QUIT_WAIT: Duration = Duration::from_secs(1);
/// Main init loop sleep.
pub const INIT_LOOP_SLEEP: Duration = Duration::from_millis(200);
/// Getty respawn delay after exit.
pub const GETTY_RESPAWN_DELAY: Duration = Duration::from_secs(1);
/// Maximum IPC frame payload size (bytes).
pub const MAX_IPC_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// Max concurrent IPC client handler threads.
pub const MAX_IPC_CLIENTS: usize = 32;
/// Max child exits retained in the central registry (bound memory).
pub const MAX_EXIT_REGISTRY_ENTRIES: usize = 4096;
/// Max live `microinit logs --follow` subscribers.
pub const MAX_LOG_FOLLOWERS: usize = 16;
/// Max live `microinit watch` subscribers. Independent of log followers.
/// Excess clients receive `{code: busy}`; the oldest is not dropped (a silent
/// drop would desynchronize mDNS / other snapshot consumers).
pub const MAX_WATCH_FOLLOWERS: usize = 8;
/// Keepalive interval on `watch` streams so clients with idle read deadlines
/// do not disconnect a quiet but healthy subscription.
pub const WATCH_HEARTBEAT: Duration = Duration::from_secs(10);
/// Poll interval while waiting for a process to exit after SIGTERM.
pub const TERMINATE_POLL: Duration = Duration::from_millis(100);
/// Per-service lifecycle event ring capacity (bounded memory).
/// Same as [`EVENT_RETURN`]: the ring only exists to feed `describe`.
pub const EVENT_RING_CAP: usize = 16;
/// How many recent lifecycle events `describe` returns (= ring capacity).
pub const EVENT_RETURN: usize = EVENT_RING_CAP;
