//! Unit/integration tests for microinit::logs

use std::io::Cursor;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use microinit::logs::*;
use microinit::protocol::{LogLevel, LogLine};

fn line(service: &str, msg: &str) -> LogLine {
    LogLine {
        ts: "t".into(),
        service: service.into(),
        level: LogLevel::Info,
        msg: msg.into(),
    }
}

#[test]
fn ring_buffer_evicts_oldest() {
    let mut buf = RingBuffer::new(3);
    buf.push(line("s", "1"));
    buf.push(line("s", "2"));
    buf.push(line("s", "3"));
    buf.push(line("s", "4"));
    let all = buf.all();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].msg, "2");
    assert_eq!(all[2].msg, "4");
    assert_eq!(buf.last_n(2)[0].msg, "3");
}

#[test]
fn ring_capacity_at_least_one() {
    let mut buf = RingBuffer::new(0);
    buf.push(line("s", "only"));
    assert_eq!(buf.all().len(), 1);
}

#[test]
fn hub_separates_services_and_mixed() {
    let hub = LogHub::new(10, None, None, None);
    hub.emit("redis", LogLevel::Stdout, "hello");
    hub.emit("alloy", LogLevel::Stderr, "warn");
    hub.emit("redis", LogLevel::Stdout, "world");

    let redis = hub.snapshot_service("redis", 10);
    assert_eq!(redis.len(), 2);
    assert_eq!(redis[1].msg, "world");
    assert!(hub.snapshot_service("missing", 5).is_empty());
    let mixed = hub.snapshot_mixed(10);
    assert_eq!(mixed.len(), 3);
    assert_eq!(hub.snapshot_mixed(1)[0].msg, "world");
}

#[test]
fn hub_writes_files_only_when_log_dir_set() {
    let dir = std::env::temp_dir().join(format!("microinit-logs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let hub = LogHub::new(10, None, None, Some(dir.clone()));
    hub.emit("redis", LogLevel::Stdout, "hello");

    let path = dir.join("redis.log");
    assert!(path.is_file());
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("hello"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hub_followers_receive_live_lines() {
    let hub = Arc::new(LogHub::new(5, None, None, None));
    let rx = hub.subscribe();
    hub.emit("svc", LogLevel::Info, "ping");
    let got = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(got.msg, "ping");
    assert_eq!(got.service, "svc");
}

#[test]
fn capture_stream_feeds_hub() {
    let hub = Arc::new(LogHub::new(5, None, None, None));
    let data = Cursor::new(b"line-a\nline-b\n".to_vec());
    capture_stream(hub.clone(), "job".into(), LogLevel::Stdout, data);
    for _ in 0..50 {
        if hub.snapshot_service("job", 10).len() >= 2 {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let lines = hub.snapshot_service("job", 10);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].msg, "line-a");
    assert_eq!(lines[1].level, LogLevel::Stdout);
}

#[test]
fn dead_follower_does_not_block_emit() {
    let hub = LogHub::new(5, None, None, None);
    let rx = hub.subscribe();
    drop(rx);
    hub.emit("s", LogLevel::Info, "x");
    let rx2 = hub.subscribe();
    hub.emit("s", LogLevel::Info, "y");
    assert_eq!(rx2.recv_timeout(Duration::from_secs(1)).unwrap().msg, "y");
}

#[test]
fn hub_routes_init_and_service_ttys_separately() {
    let dir = std::env::temp_dir().join(format!(
        "microinit-tty-split-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let service_path = dir.join("service.tty");
    let init_path = dir.join("init.tty");
    let hub = LogHub::new(
        10,
        Some(service_path.to_str().unwrap()),
        Some(init_path.to_str().unwrap()),
        None,
    );
    hub.end_boot_tee();
    hub.emit("redis", LogLevel::Stdout, "svc-line");
    hub.emit(INIT_SERVICE, LogLevel::Info, "init-line");

    let service = std::fs::read_to_string(&service_path).unwrap();
    let init = std::fs::read_to_string(&init_path).unwrap();
    assert!(service.contains("redis") && service.contains("svc-line"));
    assert!(!service.contains("init-line"));
    assert!(init.contains("microinit") && init.contains("init-line"));
    assert!(!init.contains("svc-line"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn boot_tee_mirrors_init_to_stderr_until_ended() {
    let hub = LogHub::new(5, None, None, None);
    assert!(hub.boot_tee_enabled());
    hub.emit_init(LogLevel::Info, "during-boot");
    hub.end_boot_tee();
    assert!(!hub.boot_tee_enabled());
    hub.emit_init(LogLevel::Info, "after-boot");
    let snap = hub.snapshot_service(INIT_SERVICE, 10);
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].msg, "during-boot");
    assert_eq!(snap[1].msg, "after-boot");
}
