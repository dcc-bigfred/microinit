//! Unit/integration tests for microinit::config

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use microinit::config::*;

fn minimal_svc(name: &str) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        enabled: true,
        daemon: true,
        restart: false,
        restart_backoff: 2,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: false,
        depends_on: vec![],
        cmd: Some(format!("/bin/echo-{name}")),
        start_cmd: None,
        stop_cmd: None,
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "microinit-cfg-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn resolve_cmd_fallback() {
    let svc = ServiceConfig {
        name: "x".into(),
        enabled: true,
        daemon: true,
        restart: false,
        restart_backoff: 2,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: false,
        depends_on: vec![],
        cmd: Some("/etc/init.d/redis".into()),
        start_cmd: None,
        stop_cmd: None,
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
    };
    assert_eq!(svc.resolve_start().unwrap(), "/etc/init.d/redis start");
    assert_eq!(svc.resolve_stop().unwrap(), "/etc/init.d/redis stop");
    assert_eq!(svc.resolve_restart().unwrap(), "/etc/init.d/redis restart");
}

#[test]
fn resolve_explicit_cmds_prefer_over_cmd() {
    let svc = ServiceConfig {
        name: "x".into(),
        enabled: true,
        daemon: true,
        restart: false,
        restart_backoff: 2,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: false,
        depends_on: vec![],
        cmd: Some("/ignored".into()),
        start_cmd: Some("start-me".into()),
        stop_cmd: Some("stop-me".into()),
        restart_cmd: Some("restart-me".into()),
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
    };
    assert_eq!(svc.resolve_start().unwrap(), "start-me");
    assert_eq!(svc.resolve_stop().unwrap(), "stop-me");
    assert_eq!(svc.resolve_restart().unwrap(), "restart-me");
}

#[test]
fn resolve_restart_falls_back_to_stop_and_start() {
    let svc = ServiceConfig {
        name: "x".into(),
        enabled: true,
        daemon: true,
        restart: false,
        restart_backoff: 2,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: false,
        depends_on: vec![],
        cmd: None,
        start_cmd: Some("do-start".into()),
        stop_cmd: Some("do-stop".into()),
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
    };
    assert_eq!(svc.resolve_restart().unwrap(), "do-stop && do-start");
}

#[test]
fn resolve_start_errors_without_cmds() {
    let svc = ServiceConfig {
        name: "x".into(),
        enabled: true,
        daemon: true,
        restart: false,
        restart_backoff: 2,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: false,
        depends_on: vec![],
        cmd: None,
        start_cmd: None,
        stop_cmd: None,
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
    };
    assert!(svc.resolve_start().is_err());
    assert!(svc.resolve_stop().is_err());
}

#[test]
fn is_success_custom_codes() {
    let mut svc = minimal_svc("x");
    svc.success_exit_codes = vec![0, 2];
    assert!(svc.is_success(0));
    assert!(svc.is_success(2));
    assert!(!svc.is_success(1));
}

#[test]
fn validate_rejects_bad_labels() {
    let mut cfg = Config::default();
    let mut s = minimal_svc("a");
    s.labels.insert("bad key".into(), "x".into());
    cfg.services.push(s);
    assert!(cfg.validate().is_err());

    let mut cfg = Config::default();
    let mut s = minimal_svc("a");
    s.labels.insert("created-by".into(), "bigfred".into());
    cfg.services.push(s);
    cfg.validate().unwrap();
}

#[test]
fn example_validates() {
    let cfg = example_config();
    cfg.validate().unwrap();
}

#[test]
fn validate_rejects_empty_name() {
    let mut cfg = Config::default();
    let mut s = minimal_svc("ok");
    s.name = String::new();
    cfg.services.push(s);
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_rejects_duplicate() {
    let mut cfg = Config::default();
    cfg.services.push(minimal_svc("a"));
    cfg.services.push(minimal_svc("a"));
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_rejects_restart_without_daemon() {
    let mut cfg = Config::default();
    let mut s = minimal_svc("job");
    s.daemon = false;
    s.restart = true;
    cfg.services.push(s);
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_rejects_missing_cmd() {
    let mut cfg = Config::default();
    let mut s = minimal_svc("x");
    s.cmd = None;
    s.start_cmd = None;
    cfg.services.push(s);
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_rejects_unknown_dependency() {
    let mut cfg = Config::default();
    let mut s = minimal_svc("a");
    s.depends_on = vec!["missing".into()];
    cfg.services.push(s);
    assert!(cfg.validate().is_err());
}

#[test]
fn defaults_from_partial_json() {
    let raw = r#"{"services":[{"name":"n","cmd":"/bin/true"}]}"#;
    let cfg: Config = serde_json::from_str(raw).unwrap();
    cfg.validate().unwrap();
    assert!(cfg.services[0].enabled);
    assert!(cfg.services[0].daemon);
    assert_eq!(cfg.services[0].restart_backoff, 2);
    assert_eq!(cfg.services[0].start_wait_secs, 0);
    assert_eq!(cfg.services[0].shutdown_wait_secs, 5);
    assert_eq!(cfg.logs.lines, DEFAULT_LOG_LINES);
    assert!(!cfg.logs.log_to_files);
    assert!(cfg.logs.effective_log_dir().is_none());
    assert_eq!(cfg.socket, DEFAULT_SOCKET);
}

#[test]
fn log_to_files_enables_effective_dir() {
    let mut logs = LogsConfig::default();
    assert!(!logs.log_to_files);
    assert!(logs.effective_log_dir().is_none());
    logs.log_to_files = true;
    let dir = logs.effective_log_dir().unwrap();
    assert!(dir.ends_with("logs"));
}

#[test]
fn override_merge() {
    let mut cfg = example_config();
    let mut map = HashMap::new();
    map.insert("redis".into(), false);
    apply_enabled_override(&mut cfg, &map);
    assert!(!cfg.get("redis").unwrap().enabled);
    assert!(cfg.get("network").unwrap().enabled);
}

#[test]
fn override_roundtrip() {
    let dir = temp_dir("ov");
    let path = dir.join("enabled-override.json");
    assert!(load_override(&path).unwrap().is_empty());
    let mut map = HashMap::new();
    map.insert("dropbear".into(), false);
    save_override(&path, &map).unwrap();
    let loaded = load_override(&path).unwrap();
    assert_eq!(loaded.get("dropbear"), Some(&false));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn roundtrip_json() {
    let dir = temp_dir("rt");
    let path = dir.join("microinit.json");
    let cfg = example_config();
    save_config(&path, &cfg).unwrap();
    let loaded = load_config(&path).unwrap();
    assert_eq!(loaded.services.len(), 3);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn load_or_create_writes_defaults_and_example() {
    let dir = temp_dir("loc");
    let config = dir.join("microinit.json");
    let example = dir.join("microinit.json.example");
    let override_f = dir.join("override.json");
    let dropins = dir.join("dropins");
    let cfg = load_or_create_with_dropins(&config, &example, &override_f, &dropins).unwrap();
    assert!(config.is_file());
    assert!(example.is_file());
    assert!(cfg.services.is_empty()); // default config has empty services
    assert!(!override_f.exists()); // lazy

    // Second call with override applied
    let mut map = HashMap::new();
    // empty services — write example as config then override
    save_config(&config, &example_config()).unwrap();
    map.insert("redis".into(), false);
    save_override(&override_f, &map).unwrap();
    let cfg2 = load_or_create_with_dropins(&config, &example, &override_f, &dropins).unwrap();
    assert!(!cfg2.get("redis").unwrap().enabled);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn camel_case_json_fields() {
    let raw = r#"{
      "version": 1,
      "services": [{
        "name": "x",
        "restartBackoff": 9,
        "startWaitSecs": 3,
        "shutdownWaitSecs": 7,
        "successExitCodes": [0, 3],
        "dependsOn": ["y"],
        "startCmd": "echo hi",
        "stopCmd": "true"
      }, {
        "name": "y",
        "cmd": "/bin/true"
      }]
    }"#;
    let cfg: Config = serde_json::from_str(raw).unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.get("x").unwrap().restart_backoff, 9);
    assert_eq!(cfg.get("x").unwrap().start_wait_secs, 3);
    assert_eq!(cfg.get("x").unwrap().shutdown_wait_secs, 7);
    assert_eq!(cfg.get("x").unwrap().success_exit_codes, vec![0, 3]);
    assert_eq!(cfg.get("x").unwrap().depends_on, vec!["y".to_string()]);
}

#[test]
fn parses_liveness_probe_with_defaults() {
    let raw = r#"{
      "version": 1,
      "services": [{
        "name": "net",
        "daemon": false,
        "cmd": "/etc/init.d/network",
        "livenessProbe": {
          "cmd": "/usr/sbin/configure-ethernet check"
        }
      }]
    }"#;
    let cfg: Config = serde_json::from_str(raw).unwrap();
    cfg.validate().unwrap();
    let probe = cfg.get("net").unwrap().liveness_probe.as_ref().unwrap();
    assert_eq!(
        probe.cmd.as_deref(),
        Some("/usr/sbin/configure-ethernet check")
    );
    assert_eq!(probe.success_exit_codes, vec![0]);
    assert_eq!(probe.interval, 60);
    assert_eq!(probe.timeout, 5);
    assert_eq!(probe.http_method, "GET");
    assert_eq!(probe.http_accepted_codes, vec![200]);
}

#[test]
fn parses_http_and_tcp_liveness_probes() {
    let http = r#"{
      "version": 1,
      "services": [{
        "name": "api",
        "cmd": "/bin/true",
        "livenessProbe": {
          "httpUrl": "http://127.0.0.1:8080/health",
          "httpMethod": "HEAD",
          "httpAcceptedCodes": [200, 204],
          "interval": 10,
          "timeout": 2
        }
      }]
    }"#;
    let cfg: Config = serde_json::from_str(http).unwrap();
    cfg.validate().unwrap();
    let p = cfg.get("api").unwrap().liveness_probe.as_ref().unwrap();
    assert_eq!(p.http_url.as_deref(), Some("http://127.0.0.1:8080/health"));
    assert_eq!(p.http_method, "HEAD");
    assert_eq!(p.http_accepted_codes, vec![200, 204]);
    assert!(p.cmd.is_none());

    let tcp = r#"{
      "version": 1,
      "services": [{
        "name": "redis",
        "cmd": "/bin/true",
        "livenessProbe": { "tcpAddr": "127.0.0.1:6379", "timeout": 3 }
      }]
    }"#;
    let cfg: Config = serde_json::from_str(tcp).unwrap();
    cfg.validate().unwrap();
    assert_eq!(
        cfg.get("redis")
            .unwrap()
            .liveness_probe
            .as_ref()
            .unwrap()
            .tcp_addr
            .as_deref(),
        Some("127.0.0.1:6379")
    );
}

#[test]
fn rejects_empty_liveness_probe_cmd() {
    let mut cfg = Config::default();
    let mut svc = minimal_svc("net");
    svc.daemon = false;
    svc.restart = false;
    svc.liveness_probe = Some(LivenessProbe {
        cmd: Some("  ".into()),
        http_url: None,
        tcp_addr: None,
        success_exit_codes: vec![0],
        http_accepted_codes: vec![200],
        http_method: "GET".into(),
        interval: 30,
        timeout: 5,
    });
    cfg.services.push(svc);
    assert!(cfg.validate().is_err());
}

#[test]
fn rejects_multiple_liveness_probe_kinds() {
    let raw = r#"{
      "version": 1,
      "services": [{
        "name": "x",
        "cmd": "/bin/true",
        "livenessProbe": {
          "cmd": "true",
          "tcpAddr": "127.0.0.1:1"
        }
      }]
    }"#;
    let cfg: Config = serde_json::from_str(raw).unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("exactly one"), "{err}");
}

#[test]
fn dropins_merge_lex_later_wins() {
    let dir = temp_dir("dropins");
    let config = dir.join("microinit.json");
    let example = dir.join("microinit.json.example");
    let override_f = dir.join("override.json");
    let dropins = dir.join("microinit.d/services");
    fs::create_dir_all(dropins.join("a")).unwrap();
    fs::create_dir_all(dropins.join("b/nested")).unwrap();

    let mut base = Config::default();
    base.services.push(minimal_svc("keep"));
    base.services.push({
        let mut s = minimal_svc("overlay");
        s.cmd = Some("/bin/base".into());
        s
    });
    save_config(&config, &base).unwrap();

    fs::write(
        dropins.join("a/10-overlay.json"),
        r#"{"services":[{"name":"overlay","cmd":"/bin/from-a","daemon":true}]}"#,
    )
    .unwrap();
    fs::write(
        dropins.join("b/nested/20-overlay.json"),
        r#"{"services":[{"name":"overlay","cmd":"/bin/from-b","daemon":true},{"name":"added","cmd":"/bin/added","daemon":true}]}"#,
    )
    .unwrap();

    let cfg = load_or_create_with_dropins(&config, &example, &override_f, &dropins).unwrap();
    assert_eq!(
        cfg.get("overlay").unwrap().cmd.as_deref(),
        Some("/bin/from-b")
    );
    assert!(cfg.get("added").is_some());
    assert!(cfg.get("keep").is_some());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn open_telemetry_defaults_from_json() {
    let raw = r#"{"openTelemetry":{"enable":true,"endpoint":"http://alloy:4318"}}"#;
    let cfg: Config = serde_json::from_str(raw).unwrap();
    assert!(cfg.open_telemetry.enable);
    assert_eq!(cfg.open_telemetry.endpoint, "http://alloy:4318");
    assert_eq!(cfg.open_telemetry.protocol, "http");
    assert_eq!(cfg.open_telemetry.service_name, "microinit");
    assert_eq!(cfg.open_telemetry.export_interval_secs, 15);
}
