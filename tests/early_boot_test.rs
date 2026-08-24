//! Unit/integration tests for microinit::early_boot

#![cfg(feature = "init")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use microinit::config::Paths;
use microinit::constants::{
    MAX_EARLY_BOOT_CAPTURE_BYTES, MAX_EARLY_BOOT_CAPTURE_LINES, MAX_EARLY_BOOT_LINE_BYTES,
};
use microinit::early_boot::*;
use microinit::error::Error;

fn temp_paths(label: &str) -> (Paths, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "microinit-eb-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("etc")).unwrap();
    fs::create_dir_all(dir.join("data/etc/microinit")).unwrap();
    let paths = Paths {
        config: dir.join("data/etc/microinit.json"),
        example: dir.join("data/etc/microinit.json.example"),
        override_file: dir.join("data/etc/override.json"),
        dropins_dir: dir.join("data/etc/microinit.d/services"),
        early_boot: dir.join("etc/early-boot.sh"),
        early_boot_override: dir.join("data/etc/microinit/early-boot.sh"),
        unmount: dir.join("etc/unmount.sh"),
        unmount_override: dir.join("data/etc/microinit/unmount.sh"),
    };
    (paths, dir)
}

fn write_exec(path: &Path, body: &str) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, body).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn resolve_prefers_override() {
    let (paths, dir) = temp_paths("pref");
    write_exec(&paths.early_boot, "#!/bin/sh\nexit 0\n");
    write_exec(&paths.early_boot_override, "#!/bin/sh\nexit 0\n");
    assert_eq!(
        resolve_script(&paths),
        ScriptSource::Path(paths.early_boot_override.clone())
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resolve_falls_back_to_base() {
    let (paths, dir) = temp_paths("base");
    write_exec(&paths.early_boot, "#!/bin/sh\nexit 0\n");
    assert_eq!(
        resolve_script(&paths),
        ScriptSource::Path(paths.early_boot.clone())
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resolve_embedded_when_missing() {
    let (paths, dir) = temp_paths("none");
    assert_eq!(resolve_script(&paths), ScriptSource::Embedded);
    assert!(EMBEDDED_EARLY_BOOT.contains("mount -a"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn run_uses_embedded_when_no_on_disk_script() {
    let (paths, dir) = temp_paths("emb");
    assert_eq!(resolve_script(&paths), ScriptSource::Embedded);
    let (out, res) = run_script_bytes("#!/bin/sh\nexit 0\n", "/dev/null", "/dev/null", "/dev/null");
    res.unwrap();
    assert!(out.captured);
    assert_eq!(out.exit_code, Some(0));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn run_script_bytes_failure() {
    let (out, err) = run_script_bytes("#!/bin/sh\nexit 9\n", "/dev/null", "/dev/null", "/dev/null");
    assert!(matches!(err.unwrap_err(), Error::EarlyBoot(9)));
    assert!(out.captured);
    assert_eq!(out.exit_code, Some(9));
}

#[test]
fn run_script_success_and_failure() {
    let (paths, dir) = temp_paths("run");
    write_exec(
        &paths.early_boot,
        "#!/bin/sh\ntest \"$MICROINIT_LOGS_TTY\" = /dev/ttyX \\\n  -a \"$MICROINIT_INIT_LOGS_TTY\" = /dev/ttyZ \\\n  -a -n \"$DATA_DIR\"\n",
    );
    let (out, res) = run_script(&paths.early_boot, "/dev/ttyX", "/dev/ttyZ", "/dev/ttyY");
    res.unwrap();
    assert!(out.captured);
    assert_eq!(out.exit_code, Some(0));

    write_exec(&paths.early_boot, "#!/bin/sh\nexit 7\n");
    let (out, err) = run_script(&paths.early_boot, "/dev/null", "/dev/null", "/dev/null");
    assert!(matches!(err.unwrap_err(), Error::EarlyBoot(7)));
    assert!(out.captured);
    assert_eq!(out.exit_code, Some(7));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn capture_stdout_stderr_and_final_line() {
    let (out, res) = run_script_bytes(
        "#!/bin/sh\necho stdout-a\necho stderr-b >&2\necho stdout-c\necho last-line\n",
        "/dev/null",
        "/dev/null",
        "/dev/null",
    );
    res.unwrap();
    assert!(out.captured);
    assert_eq!(
        out.lines,
        vec!["stdout-a", "stderr-b", "stdout-c", "last-line"]
    );
    assert_eq!(out.dropped, 0);
}

#[test]
fn capture_survives_nonzero_exit() {
    let (out, err) = run_script_bytes(
        "#!/bin/sh\necho boom\nexit 9\n",
        "/dev/null",
        "/dev/null",
        "/dev/null",
    );
    assert!(matches!(err.unwrap_err(), Error::EarlyBoot(9)));
    assert!(out.captured);
    assert_eq!(out.lines, vec!["boom"]);
}

#[test]
fn capture_evicts_oldest_lines() {
    let n = MAX_EARLY_BOOT_CAPTURE_LINES + 10;
    let script = format!(
        "#!/bin/sh\ni=1\nwhile [ \"$i\" -le {n} ]; do\n  echo \"line-$i\"\n  i=$((i + 1))\ndone\n"
    );
    let (out, res) = run_script_bytes(&script, "/dev/null", "/dev/null", "/dev/null");
    res.unwrap();
    assert!(out.captured);
    assert_eq!(out.dropped, 10);
    assert_eq!(out.lines.len(), MAX_EARLY_BOOT_CAPTURE_LINES);
    assert_eq!(out.lines.first().map(String::as_str), Some("line-11"));
    assert_eq!(
        out.lines.last().map(String::as_str),
        Some(format!("line-{n}").as_str())
    );
}

#[test]
fn capture_truncates_long_line() {
    let (out, res) = run_script_bytes(
        "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 5000 ]; do\n  printf x\n  i=$((i + 1))\ndone\necho\n",
        "/dev/null",
        "/dev/null",
        "/dev/null",
    );
    res.unwrap();
    assert_eq!(out.lines.len(), 1);
    assert_eq!(out.lines[0].len(), MAX_EARLY_BOOT_LINE_BYTES);
}

#[test]
fn write_captured_truncates_and_writes_header() {
    let dir = std::env::temp_dir().join(format!(
        "microinit-eb-write-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("early-boot.log");
    fs::write(&path, "OLD CONTENT\n").unwrap();

    let out = EarlyBootOutput {
        lines: vec!["hello".into(), "world".into()],
        dropped: 3,
        captured: true,
        source: "/etc/microinit/early-boot.sh".into(),
        exit_code: Some(0),
        signal: None,
    };
    let n = write_captured(&path, &out).unwrap();
    assert_eq!(n, 2);
    let body = fs::read_to_string(&path).unwrap();
    assert!(!body.contains("OLD CONTENT"), "{body}");
    assert!(
        body.starts_with("# early-boot ts=")
            && body.contains("source=/etc/microinit/early-boot.sh"),
        "{body}"
    );
    assert!(body.contains("exit=0"), "{body}");
    assert!(body.contains("... 3 earlier line(s) dropped"), "{body}");
    assert!(body.contains("hello\nworld\n"), "{body}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn write_captured_unwritable_path_is_error() {
    let dir = std::env::temp_dir().join(format!(
        "microinit-eb-nowrite-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let not_a_dir = dir.join("notdir");
    fs::write(&not_a_dir, "x").unwrap();
    let out = EarlyBootOutput {
        lines: vec!["x".into()],
        captured: true,
        source: "embedded".into(),
        exit_code: Some(0),
        ..EarlyBootOutput::default()
    };
    assert!(write_captured(&not_a_dir.join("log"), &out).is_err());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn capture_survives_invalid_utf8_and_keeps_later_lines() {
    let (out, res) = run_script_bytes(
        "#!/bin/sh\nprintf '\\377\\376\\n'\necho after-utf8\n",
        "/dev/null",
        "/dev/null",
        "/dev/null",
    );
    res.unwrap();
    assert!(out.captured);
    assert!(
        out.lines.iter().any(|l| l.contains("after-utf8")),
        "reader must not stop on invalid UTF-8; got {:?}",
        out.lines
    );
    assert!(
        out.lines.iter().any(|l| l.contains('\u{FFFD}')),
        "invalid bytes should become U+FFFD; got {:?}",
        out.lines
    );
}

#[test]
fn capture_truncates_unterminated_long_line() {
    let (out, res) = run_script_bytes(
        "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 5000 ]; do\n  printf x\n  i=$((i + 1))\ndone\n",
        "/dev/null",
        "/dev/null",
        "/dev/null",
    );
    res.unwrap();
    assert_eq!(out.lines.len(), 1);
    assert_eq!(out.lines[0].len(), MAX_EARLY_BOOT_LINE_BYTES);
}

#[test]
fn capture_evicts_by_byte_budget() {
    let line_len = 4000usize;
    let n = (MAX_EARLY_BOOT_CAPTURE_BYTES / line_len) + 20;
    let script = format!(
        "#!/bin/sh\nline=$(printf '%{line_len}s' | tr ' ' x)\ni=1\nwhile [ \"$i\" -le {n} ]; do\n  printf '%s\\n' \"$line\"\n  i=$((i + 1))\ndone\n"
    );
    let (out, res) = run_script_bytes(&script, "/dev/null", "/dev/null", "/dev/null");
    res.unwrap();
    assert!(out.captured);
    assert!(out.dropped > 0, "expected byte-budget eviction, dropped=0");
    let retained: usize = out.lines.iter().map(String::len).sum();
    assert!(
        retained <= MAX_EARLY_BOOT_CAPTURE_BYTES,
        "retained {retained} bytes over budget"
    );
    assert!(out.lines.iter().all(|l| l.len() == line_len));
}

#[test]
fn capture_records_terminating_signal() {
    let (out, err) = run_script_bytes(
        "#!/bin/sh\nkill -9 $$\n",
        "/dev/null",
        "/dev/null",
        "/dev/null",
    );
    let kill_denied = out.lines.iter().any(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("permission denied")
            || l.contains("operation not permitted")
            || l.contains("brak dostępu")
            || l.contains("brak dostepu")
    });
    if kill_denied {
        // Some test sandboxes block kill(2); the header format is covered by
        // write_captured_signal_header.
        return;
    }
    assert!(
        matches!(err.unwrap_err(), Error::EarlyBoot(1)),
        "signal death is reported as EarlyBoot(1)"
    );
    assert_eq!(out.exit_code, None, "signal death has no exit code");
    assert_eq!(out.signal, Some(9), "expected SIGKILL");
}

#[test]
fn write_captured_signal_header() {
    let dir = std::env::temp_dir().join(format!(
        "microinit-eb-sig-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("early-boot.log");
    let out = EarlyBootOutput {
        lines: vec!["killed".into()],
        captured: true,
        source: "embedded".into(),
        exit_code: None,
        signal: Some(9),
        ..EarlyBootOutput::default()
    };
    write_captured(&path, &out).unwrap();
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("exit=signal:9"), "{body}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn write_captured_unknown_exit_header() {
    let dir = std::env::temp_dir().join(format!(
        "microinit-eb-unk-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("early-boot.log");
    let out = EarlyBootOutput {
        lines: vec!["x".into()],
        captured: true,
        source: "embedded".into(),
        exit_code: None,
        signal: None,
        ..EarlyBootOutput::default()
    };
    write_captured(&path, &out).unwrap();
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("exit=unknown"), "{body}");
    let _ = fs::remove_dir_all(dir);
}
