//! Unit/integration tests for microinit::datadir

use std::path::PathBuf;
use std::sync::Mutex;

use microinit::datadir::{self, DEFAULT_ROOT, ENV_FALLBACK, ENV_PRIMARY};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_clean_env<F: FnOnce()>(f: F) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var(ENV_PRIMARY);
    std::env::remove_var(ENV_FALLBACK);
    f();
    std::env::remove_var(ENV_PRIMARY);
    std::env::remove_var(ENV_FALLBACK);
}

#[test]
fn root_defaults_to_data() {
    with_clean_env(|| {
        assert_eq!(datadir::root(), PathBuf::from(DEFAULT_ROOT));
    });
}

#[test]
fn bigfred_data_dir_wins() {
    with_clean_env(|| {
        let dir =
            std::env::temp_dir().join(format!("microinit-datadir-primary-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var(ENV_PRIMARY, &dir);
        std::env::set_var(ENV_FALLBACK, "/other");
        assert_eq!(datadir::root(), dir);
        assert_eq!(
            datadir::path(["etc", "microinit.json"]),
            dir.join("etc/microinit.json")
        );
    });
}

#[test]
fn data_dir_fallback() {
    with_clean_env(|| {
        let dir =
            std::env::temp_dir().join(format!("microinit-datadir-fallback-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var(ENV_FALLBACK, &dir);
        assert_eq!(datadir::root(), dir);
    });
}

#[test]
fn relative_env_ignored() {
    with_clean_env(|| {
        std::env::set_var(ENV_PRIMARY, "relative/path");
        assert_eq!(datadir::root(), PathBuf::from(DEFAULT_ROOT));
    });
}

#[test]
fn paths_default_honors_env() {
    with_clean_env(|| {
        let dir =
            std::env::temp_dir().join(format!("microinit-datadir-paths-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var(ENV_PRIMARY, &dir);
        let paths = microinit::config::Paths::default();
        assert_eq!(paths.config, dir.join("etc/microinit.json"));
        assert_eq!(
            paths.early_boot_override,
            dir.join("etc/microinit/early-boot.sh")
        );
        assert_eq!(
            paths.override_file,
            dir.join("etc/microinit.services.enabled-override.json")
        );
    });
}
