//! Unix socket IPC: length-prefixed JSON frames (4-byte LE length + payload).

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use crate::constants::{MAX_IPC_CLIENTS, MAX_IPC_FRAME_BYTES};
use crate::error::{Error, Result};
use crate::protocol::{Request, Response};

static ACTIVE_CLIENTS: AtomicUsize = AtomicUsize::new(0);

struct ClientSlot;

impl ClientSlot {
    fn try_acquire() -> Option<Self> {
        loop {
            let cur = ACTIVE_CLIENTS.load(Ordering::SeqCst);
            if cur >= MAX_IPC_CLIENTS {
                return None;
            }
            if ACTIVE_CLIENTS
                .compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Some(Self);
            }
        }
    }
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        ACTIVE_CLIENTS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn write_frame_to(writer: &mut impl Write, msg: &impl serde::Serialize) -> Result<()> {
    let payload = serde_json::to_vec(msg)?;
    if payload.len() > MAX_IPC_FRAME_BYTES {
        return Err(Error::Ipc(format!(
            "frame length {} exceeds max {MAX_IPC_FRAME_BYTES}",
            payload.len()
        )));
    }
    debug_assert!(payload.len() <= MAX_IPC_FRAME_BYTES);
    let len = u32::try_from(payload.len())
        .map_err(|_| Error::Ipc("frame too large for u32 length prefix".into()))?
        .to_le_bytes();
    writer.write_all(&len)?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

pub fn read_frame_from<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_IPC_FRAME_BYTES {
        return Err(Error::Ipc(format!("frame length {len} too large")));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(serde_json::from_slice(&buf)?)
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

/// Peer allowlist for the control socket (from `socketAllowUsers`).
#[derive(Debug, Clone, Default)]
pub struct IpcAllow {
    /// Extra uids allowed besides the daemon's own uid.
    pub allow_uids: Vec<u32>,
    /// When set with a non-empty allowlist: socket mode `0660`, owner `root:gid`.
    pub socket_gid: Option<u32>,
}

/// Peer credential check: daemon uid, or an entry in `allow_uids`.
fn peer_allowed(stream: &UnixStream, allow_uids: &[u32]) -> bool {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    use nix::unistd::Uid;
    match getsockopt(stream, PeerCredentials) {
        Ok(cred) => {
            let uid = cred.uid();
            uid == Uid::current().as_raw() || allow_uids.contains(&uid)
        }
        Err(_) => false,
    }
}

pub type Handler = Arc<dyn Fn(Request, &mut UnixStream) -> Result<()> + Send + Sync>;

/// Bind socket and accept clients in a background thread.
///
/// Concurrent handlers are capped at [`MAX_IPC_CLIENTS`]; excess clients receive
/// an immediate error response.
///
/// When `allow.allow_uids` is non-empty, the socket is immediately set to
/// `0660` and `chown`ed to `root:<allow.socket_gid>` (fail-closed if gid missing).
/// Otherwise the socket stays `0600` (daemon-uid-only).
pub fn serve(socket_path: &Path, handler: Handler, allow: IpcAllow) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
        }
    }
    match std::fs::remove_file(socket_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io_at(socket_path, e)),
    }
    let listener = UnixListener::bind(socket_path).map_err(|e| Error::io_at(socket_path, e))?;
    apply_socket_perms(socket_path, &allow)?;

    let path = socket_path.to_path_buf();
    let allow_uids = allow.allow_uids;
    thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(mut stream) => {
                    if !peer_allowed(&stream, &allow_uids) {
                        let _ = write_frame(
                            &mut stream,
                            &Response::Error {
                                message: "permission denied".into(),
                                code: Some("permission_denied".into()),
                            },
                        );
                        continue;
                    }
                    let Some(_slot) = ClientSlot::try_acquire() else {
                        let _ = write_frame(
                            &mut stream,
                            &Response::Error {
                                message: format!(
                                    "too many concurrent IPC clients (max {MAX_IPC_CLIENTS})"
                                ),
                                code: Some("busy".into()),
                            },
                        );
                        continue;
                    };
                    let h = handler.clone();
                    thread::spawn(move || {
                        let _slot = _slot;
                        let req: Request = match read_frame(&mut stream) {
                            Ok(r) => r,
                            Err(_) => return,
                        };
                        if let Err(e) = h(req, &mut stream) {
                            let _ = write_frame(
                                &mut stream,
                                &Response::Error {
                                    message: e.to_string(),
                                    code: e.code().map(|s| s.to_string()),
                                },
                            );
                        }
                    });
                }
                Err(_) => {
                    if !path.exists() {
                        break;
                    }
                }
            }
        }
    });
    Ok(())
}

fn apply_socket_perms(socket_path: &Path, allow: &IpcAllow) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if allow.allow_uids.is_empty() {
        let mut perms = std::fs::metadata(socket_path)
            .map_err(|e| Error::io_at(socket_path, e))?
            .permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(socket_path, perms).map_err(|e| Error::io_at(socket_path, e))?;
        return Ok(());
    }
    let gid = allow.socket_gid.ok_or_else(|| {
        Error::Config(
            "socketAllowUsers set but no socket group could be resolved".into(),
        )
    })?;
    // chmod + chown immediately after bind — no window with 0600 for allowlisted peers.
    let mut perms = std::fs::metadata(socket_path)
        .map_err(|e| Error::io_at(socket_path, e))?
        .permissions();
    perms.set_mode(0o660);
    std::fs::set_permissions(socket_path, perms).map_err(|e| Error::io_at(socket_path, e))?;
    use nix::unistd::{chown, Gid, Uid};
    chown(
        socket_path,
        Some(Uid::from_raw(0)),
        Some(Gid::from_raw(gid)),
    )
    .map_err(|e| {
        Error::io_at(
            socket_path,
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, e),
        )
    })?;
    Ok(())
}
