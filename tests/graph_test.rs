//! Unit/integration tests for microinit::graph

use std::collections::HashMap;

use microinit::config::ServiceConfig;
use microinit::error::Error;
use microinit::graph::*;

fn svc(name: &str, deps: &[&str], bg: bool) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        enabled: true,
        daemon: true,
        restart: false,
        restart_backoff: 2,
        success_exit_codes: vec![0],
        start_wait_secs: 0,
        shutdown_wait_secs: 5,
        background: bg,
        depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
        cmd: Some(format!("/bin/true-{name}")),
        start_cmd: None,
        stop_cmd: None,
        restart_cmd: None,
        env: HashMap::new(),
        cwd: "/".into(),
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
