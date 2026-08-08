//! Unit/integration tests for microinit::graph

use std::collections::{BTreeMap, HashMap};

use microinit::config::{RestartPolicy, ServiceConfig};
use microinit::error::Error;
use microinit::graph::*;

fn svc(name: &str, deps: &[&str], bg: bool) -> ServiceConfig {
    svc_prio(name, deps, bg, 100)
}

fn svc_prio(name: &str, deps: &[&str], bg: bool, order_priority: u64) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        enabled: true,
        daemon: true,
        restart_policy: RestartPolicy::None,
        restart_backoff: 2,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: bg,
        order_priority,
        depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
        cmd: Some(format!("/bin/true-{name}")),
        start_cmd: None,
        stop_cmd: None,
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
        liveness_probe: None,
        labels: BTreeMap::new(),
        security_context: None,
        #[cfg(not(target_os = "android"))]
        resolved_security: None,
    }
}

#[test]
fn topo_simple() {
    let services = vec![
        svc("c", &["b"], false),
        svc("a", &[], false),
        svc("b", &["a"], false),
    ];
    let order = topological_sort(&services).unwrap();
    assert_eq!(order, vec!["a", "b", "c"]);
}

#[test]
fn detects_cycle() {
    let services = vec![svc("a", &["b"], false), svc("b", &["a"], false)];
    assert!(matches!(topological_sort(&services), Err(Error::Cycle(_))));
}

#[test]
fn partition_foreground_and_background() {
    let services = vec![
        svc("a", &[], false),
        svc("b", &["a"], true),
        svc("c", &["a"], false),
    ];
    let (fg, bg) = partition_boot(&services).unwrap();
    assert_eq!(fg, vec!["a", "c"]);
    assert_eq!(bg, vec!["b"]);
}

#[test]
fn shutdown_is_reverse_topo() {
    let services = vec![
        svc("c", &["b"], false),
        svc("a", &[], false),
        svc("b", &["a"], false),
    ];
    assert_eq!(shutdown_order(&services).unwrap(), vec!["c", "b", "a"]);
}

#[test]
fn unknown_dependency_errors() {
    let services = vec![svc("a", &["ghost"], false)];
    assert!(matches!(topological_sort(&services), Err(Error::Config(_))));
}

#[test]
fn diamond_dependency() {
    // a -> b, a -> c, b+c -> d
    let services = vec![
        svc("a", &[], false),
        svc("b", &["a"], false),
        svc("c", &["a"], false),
        svc("d", &["b", "c"], false),
    ];
    let order = topological_sort(&services).unwrap();
    let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
    assert!(pos("a") < pos("b"));
    assert!(pos("a") < pos("c"));
    assert!(pos("b") < pos("d"));
    assert!(pos("c") < pos("d"));
}

#[test]
fn independent_roots_sorted_alphabetically() {
    let services = vec![
        svc("z", &[], false),
        svc("m", &[], false),
        svc("a", &[], false),
    ];
    assert_eq!(topological_sort(&services).unwrap(), vec!["a", "m", "z"]);
}

#[test]
fn independent_roots_sorted_by_order_priority() {
    let services = vec![
        svc_prio("cron", &[], false, 50),
        svc_prio("sysctl", &[], false, 10),
        svc_prio("watchdog", &[], false, 20),
    ];
    assert_eq!(
        topological_sort(&services).unwrap(),
        vec!["sysctl", "watchdog", "cron"]
    );
}

#[test]
fn equal_priority_falls_back_to_name() {
    let services = vec![
        svc_prio("redis", &[], false, 100),
        svc_prio("alloy", &[], false, 100),
        svc_prio("microdns", &[], false, 100),
    ];
    assert_eq!(
        topological_sort(&services).unwrap(),
        vec!["alloy", "microdns", "redis"]
    );
}

#[test]
fn depends_on_blocks_then_priority_wins_among_ready() {
    // network(30) and cron(50) ready first → network; then app(10) and cron → app, cron.
    let services = vec![
        svc_prio("network", &[], false, 30),
        svc_prio("app", &["network"], false, 10),
        svc_prio("cron", &[], false, 50),
    ];
    assert_eq!(
        topological_sort(&services).unwrap(),
        vec!["network", "app", "cron"]
    );
    assert_eq!(
        shutdown_order(&services).unwrap(),
        vec!["cron", "app", "network"]
    );
}
