//! Generic dotenv files applied to every supervised process.
//!
//! Unlike [`crate::otelenv`], keys keep their original case (`PATH` stays
//! `PATH`). Later files win. A missing file is a no-op.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use crate::datadir;

/// Image-layer default, refreshed on every flash.
pub const IMAGE_ENV_PATH: &str = "/etc/microinit/microinit.env";

/// Operator overlay under the data root (`$DATA_DIR/etc/microinit.env`).
#[must_use]
pub fn data_env_path() -> std::path::PathBuf {
    datadir::path(["etc", "microinit.env"])
}

/// Default `envFile` list: image then data (data wins).
#[must_use]
pub fn default_paths() -> Vec<String> {
    vec![
        IMAGE_ENV_PATH.to_string(),
        data_env_path().display().to_string(),
    ]
}

/// Parse KEY=value dotenv text. Comments (#) and blank lines are ignored.
/// Keys are **not** uppercased. Last duplicate wins.
#[must_use]
pub fn parse(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        out.insert(key.to_string(), value.to_string());
    }
    out
}

/// Load `path` and merge into `into` (file keys overwrite). Missing path is OK.
pub fn load_file(path: &Path, into: &mut HashMap<String, String>) {
    let data = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => return,
    };
    for (k, v) in parse(&data) {
        into.insert(k, v);
    }
}

/// Load `paths` in order; later files win. Missing files are skipped.
#[must_use]
pub fn load_paths(paths: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for p in paths {
        load_file(Path::new(p), &mut out);
    }
    out
}

static CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Replace the process-wide snapshot used by service spawn.
pub fn install(map: HashMap<String, String>) {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *g = map;
}

/// Load `paths` into the process-wide snapshot.
pub fn load_into_cache(paths: &[String]) {
    install(load_paths(paths));
}

/// Clone of the last installed env-file map (empty until [`load_into_cache`]).
#[must_use]
pub fn snapshot() -> HashMap<String, String> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_preserves_key_case() {
        let m = parse("PATH=/data/bin:/bin\nFoo=bar\n# c\nbad\nPATH=/override\n");
        assert_eq!(m.get("PATH").map(String::as_str), Some("/override"));
        assert_eq!(m.get("Foo").map(String::as_str), Some("bar"));
        assert!(!m.contains_key("FOO"));
        assert!(!m.contains_key("path"));
    }

    #[test]
    fn later_file_wins() {
        let dir = std::env::temp_dir().join(format!("envfile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.env");
        let b = dir.join("b.env");
        let mut fa = fs::File::create(&a).unwrap();
        writeln!(fa, "PATH=/from-a\nKEEP=a").unwrap();
        let mut fb = fs::File::create(&b).unwrap();
        writeln!(fb, "PATH=/from-b").unwrap();
        let m = load_paths(&[a.display().to_string(), b.display().to_string()]);
        assert_eq!(m.get("PATH").map(String::as_str), Some("/from-b"));
        assert_eq!(m.get("KEEP").map(String::as_str), Some("a"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_noop() {
        let m = load_paths(&["/no/such/microinit.env".into()]);
        assert!(m.is_empty());
    }
}
