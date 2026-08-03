//! Unit/integration tests for microinit::console

use std::io::Write;
use std::sync::{Arc, Mutex};

use microinit::console::{Console, ANSI_GREEN, ANSI_RED};

/// Shared in-memory buffer implementing Write.
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

#[test]
fn ok_and_fail_format() {
    let buf = Buf::default();
    let console = Console::from_writer(Box::new(buf.clone()));
    console.ok("redis");
    console.fail("alloy");
    let s = buf.text();
    assert!(s.contains("redis"));
    assert!(s.contains("[  OK  ]"));
    assert!(s.contains("alloy"));
    assert!(s.contains("[ FAIL ]"));
    assert!(s.contains(ANSI_GREEN));
    assert!(s.contains(ANSI_RED));
}

#[test]
fn info_and_starting() {
    let buf = Buf::default();
    let console = Console::from_writer(Box::new(buf.clone()));
    console.info("hello");
    console.starting("net");
    let s = buf.text();
    assert!(s.contains("microinit: hello"));
    assert!(s.contains("Starting net..."));
}

#[test]
fn long_name_no_panic() {
    let buf = Buf::default();
    let console = Console::from_writer(Box::new(buf.clone()));
    let name = "x".repeat(80);
    console.ok(&name);
    assert!(buf.text().contains(&name));
}

#[test]
fn open_missing_falls_back() {
    let _ = Console::open("/no/such/tty/device");
}

#[test]
fn boot_status_mirrors_to_hub() {
    use microinit::logs::{LogHub, INIT_SERVICE};
    use microinit::protocol::LogLevel;
    use std::sync::Arc;

    let dir = std::env::temp_dir().join(format!("microinit-console-hub-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let init_path = dir.join("init.tty");
    let console_path = dir.join("console.tty");
    std::fs::write(&console_path, []).unwrap();

    let hub = Arc::new(LogHub::new(
        20,
        None,
        Some(init_path.to_str().unwrap()),
        None,
    ));
    // Console mirror writes init-tty without stderr tee.
    hub.end_boot_tee();

    let console = Console::open_with_hub(console_path.to_str().unwrap(), Some(hub.clone()));
    console.starting("redis");
    console.ok("redis");
    console.fail("alloy");

    let snap = hub.snapshot_service(INIT_SERVICE, 10);
    assert!(snap.iter().any(|l| l.msg.contains("Starting redis")));
    assert!(snap.iter().any(|l| l.msg.contains("redis: OK")));
    assert!(snap
        .iter()
        .any(|l| l.level == LogLevel::Error && l.msg.contains("alloy: FAIL")));
    let init = std::fs::read_to_string(&init_path).unwrap();
    assert!(init.contains("redis: OK"));
    let _ = std::fs::remove_dir_all(dir);
}
