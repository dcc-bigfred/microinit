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

/// Peer credential check: peer uid must match the daemon's uid (root when PID 1).
fn peer_allowed(stream: &UnixStream) -> bool {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    use nix::unistd::Uid;
    match getsockopt(stream, PeerCredentials) {
        Ok(cred) => cred.uid() == Uid::current().as_raw(),
        Err(_) => false,
    }
}

pub type Handler = Arc<dyn Fn(Request, &mut UnixStream) -> Result<()> + Send + Sync>;

/// Bind socket and accept clients in a background thread.
///
/// Concurrent handlers are capped at [`MAX_IPC_CLIENTS`]; excess clients receive
/// an immediate error response.
pub fn serve(socket_path: &Path, handler: Handler) -> Result<()> {
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
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(socket_path)
        .map_err(|e| Error::io_at(socket_path, e))?
        .permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(socket_path, perms).map_err(|e| Error::io_at(socket_path, e))?;

    let path = socket_path.to_path_buf();
    thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(mut stream) => {
                    if !peer_allowed(&stream) {
                        let _ = write_frame(
                            &mut stream,
                            &Response::Error {
                                message: "permission denied".into(),
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
