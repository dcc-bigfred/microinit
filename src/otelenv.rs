//! Optional OpenTelemetry dotenv: `$DATA_DIR/etc/otel.env`.
//!
//! Existing process environment variables win — the file only sets keys that
//! are not already present. A missing file is a no-op.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use crate::config::OpenTelemetryConfig;
use crate::datadir;

/// Relative path under the data root.
pub const REL_PATH: &[&str] = &["etc", "otel.env"];

/// Absolute path to `$DATA_DIR/etc/otel.env`.
#[must_use]
pub fn default_path() -> PathBuf {
    datadir::path(REL_PATH.iter().copied())
}

/// Parse KEY=value dotenv text. Comments (#) and blank lines are ignored.
/// Keys are uppercased. Last duplicate wins.
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
        let key = key.trim().to_ascii_uppercase();
        if key.is_empty() {
            continue;
        }
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        out.insert(key, value.to_string());
    }
    out
}

/// Load `path` into the process environment for keys that are not already set.
/// Missing path is OK. Call once at boot so children inherit OTEL_* vars.
pub fn load(path: &Path) -> Result<(), String> {
    let data = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    for (k, v) in parse(&data) {
        if std::env::var_os(&k).is_some() {
            continue;
        }
        std::env::set_var(&k, &v);
    }
    Ok(())
}

/// Load the default `$DATA_DIR/etc/otel.env` path.
pub fn load_default() -> Result<(), String> {
    load(&default_path())
}

struct FileCache {
    mtime: Option<SystemTime>,
    data: HashMap<String, String>,
}

static FILE_CACHE: LazyLock<Mutex<FileCache>> = LazyLock::new(|| {
    Mutex::new(FileCache {
        mtime: None,
        data: HashMap::new(),
    })
});

/// Read otel.env without mutating the process environment (mtime-cached).
fn read_file_map() -> HashMap<String, String> {
    let path = default_path();
    let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());

    let mut cache = FILE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache.mtime == mtime {
        return cache.data.clone();
    }
    cache.mtime = mtime;
    cache.data = match fs::read_to_string(&path) {
        Ok(s) => parse(&s),
        Err(_) => HashMap::new(),
    };
    cache.data.clone()
}

/// Lookup: process env wins, then otel.env file values.
fn lookup(file: &HashMap<String, String>, key: &str) -> Option<String> {
    if let Ok(v) = std::env::var(key) {
        return Some(v);
    }
    file.get(key).cloned()
}

/// Overlay process env and `$DATA_DIR/etc/otel.env` onto a JSON base config.
///
/// Re-reads the file when its modification time changes so edits apply without
/// restarting microinit. Does not mutate the process environment (safe from
/// the metrics thread).
#[must_use]
pub fn overlay_config(base: OpenTelemetryConfig) -> OpenTelemetryConfig {
    let file = read_file_map();
    let mut cfg = base;

    if let Some(v) = lookup(&file, "ENABLE_TELEMETRY") {
        cfg.enable = is_truthy(&v);
    }
    // OTEL_SDK_DISABLED can only disable telemetry; it never enables export when
    // ENABLE_TELEMETRY / JSON left enable=false.
    if let Some(v) = lookup(&file, "OTEL_SDK_DISABLED") {
        if is_truthy(&v) {
            cfg.enable = false;
        }
    }

    if let Some(ep) = lookup(&file, "OTEL_EXPORTER_OTLP_ENDPOINT") {
        let ep = ep.trim();
        if !ep.is_empty() {
            cfg.endpoint = normalize_http_endpoint(ep);
        }
    }

    if let Some(proto) = lookup(&file, "OTEL_EXPORTER_OTLP_PROTOCOL") {
        let proto = proto.trim();
        if !proto.is_empty() {
            cfg.protocol = match proto {
                "http/protobuf" | "http/json" | "http" => "http".into(),
                other => other.to_string(),
            };
        }
    }

    if let Some(name) = lookup(&file, "OTEL_SERVICE_NAME") {
        let name = name.trim();
        if !name.is_empty() {
            cfg.service_name = name.to_string();
        }
    }

    if let Some(ms) = lookup(&file, "OTEL_METRIC_EXPORT_INTERVAL") {
        if let Ok(ms) = ms.trim().parse::<u64>() {
            cfg.export_interval_secs = (ms / 1000).max(1);
        }
    }

    if let Some(hdrs) = lookup(&file, "OTEL_EXPORTER_OTLP_HEADERS") {
        let parsed = parse_headers(&hdrs);
        if !parsed.is_empty() {
            cfg.headers = parsed;
        }
    }

    cfg
}

fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Ensure microinit's HTTP exporter gets a URL with scheme.
fn normalize_http_endpoint(raw: &str) -> String {
    let s = raw.trim();
    if s.starts_with("http://") || s.starts_with("https://") {
        return s.to_string();
    }
    // Bare host:port from BigFred gRPC style — prefer HTTP sibling :4318 when
    // the port looks like the gRPC default; otherwise prefix http:// as-is.
    if let Some((host, port)) = s.rsplit_once(':') {
        if port == "4317" {
            return format!("http://{host}:4318");
        }
    }
    format!("http://{s}")
}

fn parse_headers(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        out.insert(k.to_string(), v.trim().to_string());
    }
    out
}

