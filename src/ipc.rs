//! Unix socket IPC: length-prefixed JSON frames (4-byte LE length + payload).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use bigfred_shared_daemon::ipc::{
    read_frame_with_limit, write_frame_with_limit, AcceptPolicy, Auth, BindError, BindOptions,
    Command, Connection, ErrorHandler, IpcError, RejectReason, Router, SessionMode,
};
use serde_json::Value;

use crate::constants::{MAX_IPC_CLIENTS, MAX_IPC_FRAME_BYTES};
use crate::error::{Error, Result};
use crate::protocol::{Request, Response};

pub fn write_frame_to(writer: &mut impl Write, msg: &impl serde::Serialize) -> Result<()> {
    write_frame_with_limit(writer, msg, MAX_IPC_FRAME_BYTES).map_err(map_frame)
}

pub fn read_frame_from<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    read_frame_with_limit(reader, MAX_IPC_FRAME_BYTES).map_err(map_frame)
}

pub fn write_frame(stream: &mut UnixStream, msg: &impl serde::Serialize) -> Result<()> {
    write_frame_to(stream, msg)
}

pub fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    read_frame_from(stream)
}

pub fn connect(socket_path: &Path) -> Result<UnixStream> {
    UnixStream::connect(socket_path).map_err(|e| {
        Error::Ipc(format!(
            "cannot connect to {}: {e} (is microinit init running?)",
            socket_path.display()
        ))
    })
}

pub fn request(socket_path: &Path, req: &Request) -> Result<Response> {
    let mut stream = connect(socket_path)?;
    write_frame(&mut stream, req)?;
    read_frame(&mut stream)
}

fn map_frame(e: bigfred_shared_daemon::ipc::FrameError) -> Error {
    Error::Ipc(e.to_string())
}

fn map_bind(e: BindError) -> Error {
    match e {
        BindError::AlreadyRunning {
            process_name,
            location,
            ..
        } => Error::Ipc(format!("{process_name} already running at {location}")),
        BindError::Io { path, source } => Error::io_at(path, source),
    }
}

/// Peer allowlist for the control socket (from `socketAllowUsers`).
#[derive(Debug, Clone)]
pub struct IpcAllow {
    /// Daemon uid (always allowed). Captured at config resolve time.
    pub daemon_uid: u32,
    /// Extra uids allowed besides [`Self::daemon_uid`].
    pub allow_uids: Vec<u32>,
    /// When set with a non-empty allowlist: socket mode `0660`, owner
    /// `daemon_uid:socket_gid`.
    pub socket_gid: Option<u32>,
}

impl Default for IpcAllow {
    fn default() -> Self {
        Self {
            daemon_uid: nix::unistd::Uid::current().as_raw(),
            allow_uids: Vec::new(),
            socket_gid: None,
        }
    }
}

/// Bind socket and accept clients in a background thread.
pub type Handler = Arc<dyn Fn(Request, &mut UnixStream) -> Result<()> + Send + Sync>;

struct HandlerState {
    handler: Handler,
}

macro_rules! handler_cmd {
    ($ty:ident, $name:literal) => {
        struct $ty;
        impl Command<HandlerState> for $ty {
            fn name(&self) -> &'static str {
                $name
            }
            fn execute(
                &self,
                state: &HandlerState,
                body: Value,
                conn: &mut Connection,
            ) -> std::result::Result<(), IpcError> {
                dispatch_handler(state, body, conn)
            }
        }
    };
}

handler_cmd!(ListCmd, "list");
handler_cmd!(StartCmd, "start");
handler_cmd!(StopCmd, "stop");
handler_cmd!(RestartCmd, "restart");
handler_cmd!(StatusCmd, "status");
handler_cmd!(DescribeCmd, "describe");
handler_cmd!(EnableCmd, "enable");
handler_cmd!(LogsCmd, "logs");
handler_cmd!(InfoCmd, "info");
handler_cmd!(ShutdownCmd, "shutdown");
handler_cmd!(WatchCmd, "watch");

fn dispatch_handler(
    state: &HandlerState,
    body: Value,
    conn: &mut Connection,
) -> std::result::Result<(), IpcError> {
    let req: Request = serde_json::from_value(body).map_err(|e| IpcError::Other(e.to_string()))?;
    (state.handler)(req, conn.stream()).map_err(|e| IpcError::Other(e.to_string()))
}

struct InitHooks;

impl ErrorHandler<HandlerState> for InitHooks {
    fn unknown(
        &self,
        _state: &HandlerState,
        type_name: &str,
        _body: &Value,
        conn: &mut Connection,
    ) {
        let _ = conn.reply(&Response::Error {
            message: format!("unknown variant `{type_name}`"),
            code: None,
        });
    }
    fn error(&self, _state: &HandlerState, err: &IpcError, conn: &mut Connection) {
        let _ = conn.reply(&Response::Error {
            message: err.to_string(),
            code: None,
        });
    }
    fn reject(&self, _state: &HandlerState, reason: RejectReason, conn: &mut Connection) {
        match reason {
            RejectReason::Auth => {
                let _ = conn.reply(&Response::Error {
                    message: "permission denied".into(),
                    code: Some("permission_denied".into()),
                });
            }
            RejectReason::Busy => {
                let _ = conn.reply(&Response::Error {
                    message: format!("too many concurrent IPC clients (max {MAX_IPC_CLIENTS})"),
                    code: Some("busy".into()),
                });
            }
        }
    }
}

fn handler_router() -> std::result::Result<Router<HandlerState>, Error> {
    let mut router = Router::new();
    router
        .add(ListCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(StartCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(StopCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(RestartCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(StatusCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(DescribeCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(EnableCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(LogsCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(InfoCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(ShutdownCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    router
        .add(WatchCmd)
        .map_err(|e| Error::Other(e.to_string()))?;
    Ok(router)
}

/// Bind the control socket without stealing a live daemon's inode.
pub fn serve(socket_path: &Path, handler: Handler, allow: IpcAllow) -> Result<()> {
    let (mode, chown) = if allow.allow_uids.is_empty() {
        (0o600, None)
    } else {
        let gid = allow.socket_gid.ok_or_else(|| {
            Error::Config("socketAllowUsers set but no socket group could be resolved".into())
        })?;
        (0o660, Some((allow.daemon_uid, gid)))
    };
    let state = Arc::new(HandlerState { handler });
    bigfred_shared_daemon::ipc::serve_background(
        BindOptions {
            path: socket_path.to_path_buf(),
            mode,
            chown,
            process_name: "microinit",
        },
        AcceptPolicy {
            auth: Auth::PeerUid {
                daemon_uid: allow.daemon_uid,
                allow_uids: allow.allow_uids,
            },
            session: SessionMode::OneShot,
            max_clients: Some(MAX_IPC_CLIENTS),
            max_frame: MAX_IPC_FRAME_BYTES,
        },
        handler_router()?,
        InitHooks,
        state,
    )
    .map_err(map_bind)
}
