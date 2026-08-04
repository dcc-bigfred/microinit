//! Unit/integration tests for microinit::unmount

#![cfg(feature = "init")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use microinit::config::Paths;
use microinit::error::Error;
use microinit::unmount::*;

fn temp_paths(label: &str) -> (Paths, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "microinit-um-{}-{}-{}",
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
    write_exec(&paths.unmount, "#!/bin/sh\nexit 0\n");
    write_exec(&paths.unmount_override, "#!/bin/sh\nexit 0\n");
    assert_eq!(
        resolve_script(&paths),
        ScriptSource::Path(paths.unmount_override.clone())
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resolve_falls_back_to_base() {
    let (paths, dir) = temp_paths("base");
    write_exec(&paths.unmount, "#!/bin/sh\nexit 0\n");
    assert_eq!(
        resolve_script(&paths),
        ScriptSource::Path(paths.unmount.clone())
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resolve_embedded_when_missing() {
    let (paths, dir) = temp_paths("none");
    assert_eq!(resolve_script(&paths), ScriptSource::Embedded);
    assert!(EMBEDDED_UNMOUNT.contains("umount"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn run_uses_embedded_when_no_on_disk_script() {
    let (paths, dir) = temp_paths("emb");
    assert_eq!(resolve_script(&paths), ScriptSource::Embedded);
    run_script_bytes("#!/bin/sh\nexit 0\n", "/dev/null", "/dev/null", "/dev/null").unwrap();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn run_script_bytes_failure() {
    let err =
        run_script_bytes("#!/bin/sh\nexit 9\n", "/dev/null", "/dev/null", "/dev/null").unwrap_err();
    assert!(matches!(err, Error::Unmount(9)));
}

#[test]
fn run_script_success_and_failure() {
    let (paths, dir) = temp_paths("run");
    write_exec(
        &paths.unmount,
        "#!/bin/sh\ntest \"$MICROINIT_LOGS_TTY\" = /dev/ttyX \\\n  -a \"$MICROINIT_INIT_LOGS_TTY\" = /dev/ttyZ \\\n  -a -n \"$DATA_DIR\"\n",
    );
    run_script(&paths.unmount, "/dev/ttyX", "/dev/ttyZ", "/dev/ttyY").unwrap();

    write_exec(&paths.unmount, "#!/bin/sh\nexit 7\n");
    let err = run_script(&paths.unmount, "/dev/null", "/dev/null", "/dev/null").unwrap_err();
    assert!(matches!(err, Error::Unmount(7)));
    let _ = fs::remove_dir_all(dir);
}
