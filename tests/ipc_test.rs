//! Unit/integration tests for microinit::ipc

use std::collections::BTreeMap;
use std::io::Cursor;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread;

use microinit::ipc::*;
use microinit::protocol::{Request, Response, ServiceState, ServiceStatus};

#[test]
fn frame_roundtrip_cursor() {
    let mut buf = Vec::new();
    write_frame_to(
        &mut buf,
        &Request::Start {
            name: "redis".into(),
            force: false,
        },
    )
    .unwrap();
    let mut cur = Cursor::new(buf);
    let req: Request = read_frame_from(&mut cur).unwrap();
    match req {
        Request::Start { name, force } => {
            assert_eq!(name, "redis");
            assert!(!force);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn frame_roundtrip_unix_pair() {
    let (mut a, mut b) = UnixStream::pair().unwrap();
    let handle = thread::spawn(move || {
        let resp: Response = read_frame(&mut b).unwrap();
        match resp {
            Response::List { services } => {
                assert_eq!(services.len(), 1);
                assert_eq!(services[0].name, "net");
            }
            other => panic!("unexpected {other:?}"),
        }
        write_frame(&mut b, &Response::Ok { message: None }).unwrap();
    });
    write_frame(
        &mut a,
        &Response::List {
            services: vec![ServiceStatus {
                name: "net".into(),
                state: ServiceState::Running,
                pid: Some(42),
                restarts: 0,
                liveness_failures: 0,
                enabled: true,
                labels: BTreeMap::new(),
            }],
        },
    )
    .unwrap();
    let ack: Response = read_frame(&mut a).unwrap();
    assert!(matches!(ack, Response::Ok { message: None }));
    handle.join().unwrap();
}

#[test]
fn reject_oversized_length() {
    let mut data = (16 * 1024 * 1024 + 1_u32).to_le_bytes().to_vec();
    data.extend_from_slice(&[0u8; 8]);
    let mut cur = Cursor::new(data);
    let err = read_frame_from::<Request>(&mut cur).unwrap_err();
    assert!(err.to_string().contains("too large"));
}

#[test]
fn serve_list_roundtrip() {
    let dir = std::env::temp_dir().join(format!("microinit-ipc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("test.sock");

    serve(
        &sock,
        Arc::new(|req, stream| {
            match req {
                Request::List => write_frame(stream, &Response::List { services: vec![] })?,
                _ => write_frame(
                    stream,
                    &Response::Error {
                        message: "no".into(),
                        code: None,
                    },
                )?,
            }
            Ok(())
        }),
    )
    .unwrap();

    thread::sleep(std::time::Duration::from_millis(50));
    let resp = request(&sock, &Request::List).unwrap();
    assert!(matches!(resp, Response::List { .. }));
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_dir_all(&dir);
}
