//! Unit/integration tests for microinit::version

use std::fs;
use std::process::Command;

use microinit::version::{info, read_section_from, SECTION_NAME};

#[test]
fn info_defaults_without_section() {
    let i = info();
    // Test binary has no release section.
    assert_eq!(i.version, "dev");
    assert!(i.tag_commit.is_empty());
    assert!(!i.build_commit.is_empty());
}

#[test]
fn read_section_objcopy_roundtrip() {
    if Command::new("objcopy").arg("--version").output().is_err() {
        return;
    }
    let src = std::env::current_exe().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "microinit-ver-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let dst = dir.join("binary");
    fs::copy(&src, &dst).unwrap();

    let section_file = dir.join("section.json");
    fs::write(&section_file, r#"{"version":"v1.2.3","commit":"abc1234"}"#).unwrap();
    let _ = Command::new("objcopy")
        .args(["--remove-section", SECTION_NAME, dst.to_str().unwrap()])
        .status();
    let st = Command::new("objcopy")
        .args([
            "--add-section",
            &format!("{SECTION_NAME}={}", section_file.display()),
            dst.to_str().unwrap(),
        ])
        .status()
        .expect("objcopy");
    assert!(st.success());

    let (v, c) = read_section_from(&dst).expect("section");
    assert_eq!(v, "v1.2.3");
    assert_eq!(c, "abc1234");

    let _ = fs::remove_dir_all(&dir);
}
