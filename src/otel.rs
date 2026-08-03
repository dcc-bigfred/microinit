//! Optional OpenTelemetry metrics exporter (OTLP/HTTP JSON → Grafana Alloy).
//!
//! Enabled by default (`feature = "otel"`). Disable with `--no-default-features`.
//! Runtime-gated by `openTelemetry.enable`.
//! Designed for `panic = "abort"`: no unwrap/expect/indexing panics.

#![cfg(feature = "otel")]

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::config::OpenTelemetryConfig;
use crate::logs::{LogHub, INIT_SERVICE};
use crate::protocol::LogLevel;
use crate::supervisor::Supervisor;

const INITIAL_SLEEP: Duration = Duration::from_secs(10);
const RETRY_SLEEP: Duration = Duration::from_secs(30);

/// Spawn the OTel export thread when `cfg.enable` is true.
pub fn maybe_spawn(
    supervisor: Arc<Supervisor>,
    cfg: OpenTelemetryConfig,
    hub: Arc<LogHub>,
) -> Option<Arc<AtomicBool>> {
    if !cfg.enable {
        return None;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thr = Arc::clone(&stop);
    let hub_thr = hub.clone();
    if let Err(e) = thread::Builder::new()
        .name("otel-metrics".into())
        .spawn(move || run_loop(supervisor, hub_thr, stop_thr))
    {
        hub.emit(
            INIT_SERVICE,
            LogLevel::Warn,
            format!("otel: failed to spawn thread: {e}"),
        );
        return None;
    }
    hub.emit(
        INIT_SERVICE,
        LogLevel::Info,
        format!(
            "otel: metrics thread started (endpoint {}, initial sleep {}s)",
            cfg.endpoint,
            INITIAL_SLEEP.as_secs()
        ),
    );
    Some(stop)
}

fn run_loop(supervisor: Arc<Supervisor>, hub: Arc<LogHub>, stop: Arc<AtomicBool>) {
    thread::sleep(INITIAL_SLEEP);
    let mut cpu_samples: HashMap<i32, (u64, Instant)> = HashMap::new();

    while !stop.load(Ordering::SeqCst) {
        let cfg = supervisor.open_telemetry();
        if !cfg.enable {
            thread::sleep(RETRY_SLEEP);
            continue;
        }

        let interval = Duration::from_secs(cfg.export_interval_secs.max(1));
        match collect_and_export(&supervisor, &cfg, &mut cpu_samples) {
            Ok(()) => {
                // Success path: wait export interval (interruptible-ish via stop flag).
                let deadline = Instant::now() + interval;
                while Instant::now() < deadline {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
            }
            Err(e) => {
                hub.emit(
                    INIT_SERVICE,
                    LogLevel::Warn,
                    format!(
                        "otel: export failed: {e}; retry in {}s",
                        RETRY_SLEEP.as_secs()
                    ),
                );
                let deadline = Instant::now() + RETRY_SLEEP;
                while Instant::now() < deadline {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
}

fn collect_and_export(
    supervisor: &Supervisor,
    cfg: &OpenTelemetryConfig,
    cpu_samples: &mut HashMap<i32, (u64, Instant)>,
) -> Result<(), String> {
    let now_nano = system_time_nanos()?;
    let ts = now_nano.as_str();
    let mut metrics: Vec<Value> = Vec::new();

    let host = host_stats()?;

    metrics.push(gauge(
        "microinit_system_uptime_seconds",
        host.uptime,
        &[],
        ts,
    ));
    metrics.push(gauge("microinit_cpu_count", host.ncpu as f64, &[], ts));
    let load_ratio = if host.ncpu > 0 {
        host.load1 / host.ncpu as f64
    } else {
        0.0
    };
    metrics.push(gauge("microinit_cpu_load_ratio", load_ratio, &[], ts));
    metrics.push(gauge(
        "microinit_memory_free_bytes",
        host.mem_free as f64,
        &[],
        ts,
    ));
    metrics.push(gauge(
        "microinit_memory_used_bytes",
        host.mem_used as f64,
        &[],
        ts,
    ));
    metrics.push(gauge(
        "microinit_swap_used_bytes",
        host.swap_used as f64,
        &[],
        ts,
    ));
    metrics.push(gauge(
        "microinit_swap_free_bytes",
        host.swap_free as f64,
        &[],
        ts,
    ));

    let mut restart_points = Vec::new();
    let mut liveness_points = Vec::new();
    let mut uptime_points = Vec::new();
    let mut cpu_points = Vec::new();
    let mut mem_points = Vec::new();

    for svc in supervisor.metrics_snapshot() {
        let attrs = vec![("service", svc.name.as_str())];
        restart_points.push(sum_point(svc.restarts as f64, &attrs, ts));
        liveness_points.push(sum_point(svc.liveness_failures as f64, &attrs, ts));
        uptime_points.push(gauge_point(svc.uptime_secs, &attrs, ts));

        if let Some(pid) = svc.pid {
            if let Some(rss) = read_rss_bytes(pid) {
                mem_points.push(gauge_point(rss as f64, &attrs, ts));
            }
            if let Some(ratio) = service_cpu_ratio(pid, cpu_samples) {
                cpu_points.push(gauge_point(ratio, &attrs, ts));
            }
        }
    }

    metrics.push(sum_metric(
        "microinit_service_restarts_total",
        restart_points,
    ));
    metrics.push(sum_metric(
        "microinit_service_liveness_failures_total",
        liveness_points,
    ));
    metrics.push(gauge_metric(
        "microinit_service_uptime_seconds",
        uptime_points,
    ));
    if !cpu_points.is_empty() {
        metrics.push(gauge_metric("microinit_service_cpu_ratio", cpu_points));
    }
    if !mem_points.is_empty() {
        metrics.push(gauge_metric("microinit_service_memory_bytes", mem_points));
    }

    let body = json!({
        "resourceMetrics": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": cfg.service_name}}
                ]
            },
            "scopeMetrics": [{
                "scope": {"name": "microinit", "version": env!("CARGO_PKG_VERSION")},
                "metrics": metrics
            }]
        }]
    });

    export_http(cfg, &body)
}

fn export_http(cfg: &OpenTelemetryConfig, body: &Value) -> Result<(), String> {
    let url = metrics_url(&cfg.endpoint, &cfg.protocol)?;
    let mut req = ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(10));
    for (k, v) in &cfg.headers {
        req = req.set(k, v);
    }
    match req.send_json(body) {
        Ok(resp) => {
            let status = resp.status();
            if (200..300).contains(&status) {
                Ok(())
            } else {
                Err(format!("HTTP {status}"))
            }
        }
        Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {code}")),
        Err(e) => Err(e.to_string()),
    }
}

fn metrics_url(endpoint: &str, protocol: &str) -> Result<String, String> {
    let proto = protocol.to_ascii_lowercase();
    if proto == "grpc" {
        return Err("protocol grpc not supported in this build (use http)".into());
    }
    let base = endpoint.trim_end_matches('/');
    if base.ends_with("/v1/metrics") {
        Ok(base.to_string())
    } else {
        Ok(format!("{base}/v1/metrics"))
    }
}

fn system_time_nanos() -> Result<String, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .map_err(|e| e.to_string())
}

fn gauge(name: &str, value: f64, attrs: &[(&str, &str)], ts: &str) -> Value {
    gauge_metric(name, vec![gauge_point(value, attrs, ts)])
}

fn gauge_metric(name: &str, data_points: Vec<Value>) -> Value {
    json!({
        "name": name,
        "gauge": { "dataPoints": data_points }
    })
}

fn gauge_point(value: f64, attrs: &[(&str, &str)], ts: &str) -> Value {
    json!({
        "asDouble": value,
        "timeUnixNano": ts,
        "attributes": attrs_json(attrs)
    })
}

fn sum_metric(name: &str, data_points: Vec<Value>) -> Value {
    json!({
        "name": name,
        "sum": {
            "aggregationTemporality": 2,
            "isMonotonic": true,
            "dataPoints": data_points
        }
    })
}

fn sum_point(value: f64, attrs: &[(&str, &str)], ts: &str) -> Value {
    json!({
        "asDouble": value,
        "timeUnixNano": ts,
        "attributes": attrs_json(attrs)
    })
}

fn attrs_json(attrs: &[(&str, &str)]) -> Vec<Value> {
    attrs
        .iter()
        .map(|(k, v)| {
            json!({
                "key": k,
                "value": { "stringValue": v }
            })
        })
        .collect()
}

struct HostStats {
    uptime: f64,
    load1: f64,
    ncpu: usize,
    mem_free: u64,
    mem_used: u64,
    swap_used: u64,
    swap_free: u64,
}

fn host_stats() -> Result<HostStats, String> {
    let uptime = read_uptime()?;
    let load1 = read_loadavg1()?;
    let ncpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let (mem_total, mem_avail, swap_total, swap_free) = read_meminfo()?;
    let mem_free = mem_avail;
    let mem_used = mem_total.saturating_sub(mem_avail);
    let swap_used = swap_total.saturating_sub(swap_free);
    Ok(HostStats {
        uptime,
        load1,
        ncpu,
        mem_free,
        mem_used,
        swap_used,
        swap_free,
    })
}

fn read_uptime() -> Result<f64, String> {
    let data = fs::read_to_string("/proc/uptime").map_err(|e| e.to_string())?;
    let first = data.split_whitespace().next().ok_or("empty /proc/uptime")?;
    first.parse::<f64>().map_err(|e| e.to_string())
}

fn read_loadavg1() -> Result<f64, String> {
    let data = fs::read_to_string("/proc/loadavg").map_err(|e| e.to_string())?;
    let first = data
        .split_whitespace()
        .next()
        .ok_or("empty /proc/loadavg")?;
    first.parse::<f64>().map_err(|e| e.to_string())
}

fn read_meminfo() -> Result<(u64, u64, u64, u64), String> {
    let data = fs::read_to_string("/proc/meminfo").map_err(|e| e.to_string())?;
    let mut mem_total = 0u64;
    let mut mem_avail = 0u64;
    let mut mem_free = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;
    for line in data.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("");
        let val = parts
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            .saturating_mul(1024);
        match key {
            "MemTotal:" => mem_total = val,
            "MemAvailable:" => mem_avail = val,
            "MemFree:" => mem_free = val,
            "SwapTotal:" => swap_total = val,
            "SwapFree:" => swap_free = val,
            _ => {}
        }
    }
    if mem_avail == 0 {
        mem_avail = mem_free;
    }
    Ok((mem_total, mem_avail, swap_total, swap_free))
}

fn read_rss_bytes(pid: i32) -> Option<u64> {
    let data = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kb.saturating_mul(1024));
        }
    }
    None
}

/// CPU time in clock ticks (utime + stime) from `/proc/<pid>/stat`.
fn read_cpu_ticks(pid: i32) -> Option<u64> {
    let data = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm can contain spaces/parens — find last ')' then split.
    let after = data.rfind(')')?;
    let rest = data.get(after + 1..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // fields[11]=utime, fields[12]=stime (0-based after comm)
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    Some(utime.saturating_add(stime))
}

fn service_cpu_ratio(pid: i32, samples: &mut HashMap<i32, (u64, Instant)>) -> Option<f64> {
    let ticks = read_cpu_ticks(pid)?;
    let now = Instant::now();
    let ratio = if let Some((prev_ticks, prev_t)) = samples.get(&pid).copied() {
        let dt = now.saturating_duration_since(prev_t).as_secs_f64();
        if dt <= 0.0 {
            0.0
        } else {
            let tick_hz = ticks_per_second();
            let d_ticks = ticks.saturating_sub(prev_ticks) as f64;
            (d_ticks / tick_hz) / dt
        }
    } else {
        0.0
    };
    samples.insert(pid, (ticks, now));
    Some(ratio)
}

fn ticks_per_second() -> f64 {
    // Linux default USER_HZ; avoids unsafe sysconf under crate-level deny(unsafe_code).
    100.0
}
