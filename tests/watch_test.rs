//! Unit/integration tests for microinit::watch

use std::collections::BTreeMap;
use std::time::Duration;

use microinit::constants::MAX_WATCH_FOLLOWERS;
use microinit::protocol::{ServiceState, ServiceStatus};
use microinit::watch::{WaitOutcome, WatchHub};

fn status(name: &str, state: ServiceState, pid: Option<i32>, port: Option<&str>) -> ServiceStatus {
    let mut labels = BTreeMap::new();
    if let Some(p) = port {
        labels.insert("microdns-port".into(), p.into());
    }
    ServiceStatus {
        name: name.into(),
        state,
        pid,
        restarts: 0,
        liveness_failures: 0,
        enabled: true,
        labels,
    }
}

#[test]
fn subscribe_seeds_and_filters() {
    let hub = WatchHub::new();
    let sub = hub
        .try_subscribe(vec!["microdns-port".into()])
        .expect("subscribe");
    hub.publish(vec![
        status("redis", ServiceState::Running, Some(1), None),
        status("web", ServiceState::Running, Some(2), Some("8080")),
    ]);
    match sub.wait_timeout(0, Duration::from_secs(1)) {
        WaitOutcome::Snapshot { gen, services } => {
            assert!(gen > 0);
            assert_eq!(services.len(), 1);
            assert_eq!(services[0].name, "web");
        }
        WaitOutcome::Timeout => panic!("expected snapshot"),
    }
}

#[test]
fn pid_change_does_not_wake() {
    let hub = WatchHub::new();
    let sub = hub.try_subscribe(vec![]).expect("subscribe");
    hub.publish(vec![status(
        "web",
        ServiceState::Running,
        Some(1),
        Some("8080"),
    )]);
    let WaitOutcome::Snapshot { gen, .. } = sub.wait_timeout(0, Duration::from_secs(1)) else {
        panic!("expected first snapshot");
    };
    hub.publish(vec![status(
        "web",
        ServiceState::Running,
        Some(99),
        Some("8080"),
    )]);
    match sub.wait_timeout(gen, Duration::from_millis(80)) {
        WaitOutcome::Timeout => {}
        WaitOutcome::Snapshot { .. } => panic!("pid-only change must not wake"),
    }
}

#[test]
fn state_change_wakes_with_latest() {
    let hub = WatchHub::new();
    let sub = hub.try_subscribe(vec![]).expect("subscribe");
    hub.publish(vec![status("web", ServiceState::Starting, None, None)]);
    let WaitOutcome::Snapshot { gen, services } = sub.wait_timeout(0, Duration::from_secs(1))
    else {
        panic!("expected snapshot");
    };
    assert_eq!(services[0].state, ServiceState::Starting);
    hub.publish(vec![status("web", ServiceState::Running, Some(1), None)]);
    match sub.wait_timeout(gen, Duration::from_secs(1)) {
        WaitOutcome::Snapshot { services, .. } => {
            assert_eq!(services[0].state, ServiceState::Running);
        }
        WaitOutcome::Timeout => panic!("expected state-change snapshot"),
    }
}

#[test]
fn coalesces_to_latest_while_unread() {
    let hub = WatchHub::new();
    let sub = hub.try_subscribe(vec![]).expect("subscribe");
    hub.publish(vec![status("web", ServiceState::Starting, None, None)]);
    hub.publish(vec![status("web", ServiceState::Running, Some(1), None)]);
    hub.publish(vec![status("web", ServiceState::Stopped, None, None)]);
    match sub.wait_timeout(0, Duration::from_secs(1)) {
        WaitOutcome::Snapshot { services, .. } => {
            assert_eq!(services.len(), 1);
            assert_eq!(services[0].state, ServiceState::Stopped);
        }
        WaitOutcome::Timeout => panic!("expected coalesced snapshot"),
    }
}

#[test]
fn cap_returns_none() {
    let hub = WatchHub::new();
    let mut held = Vec::new();
    for _ in 0..MAX_WATCH_FOLLOWERS {
        held.push(hub.try_subscribe(vec![]).expect("slot"));
    }
    assert!(hub.try_subscribe(vec![]).is_none());
    assert_eq!(hub.follower_count(), MAX_WATCH_FOLLOWERS);
    drop(held);
    assert_eq!(hub.follower_count(), 0);
    assert!(hub.try_subscribe(vec![]).is_some());
}

#[test]
fn publish_is_noop_without_followers() {
    let hub = WatchHub::new();
    hub.publish(vec![status("web", ServiceState::Running, Some(1), None)]);
    assert_eq!(hub.follower_count(), 0);
}

#[test]
fn removed_service_disappears() {
    let hub = WatchHub::new();
    let sub = hub.try_subscribe(vec![]).expect("subscribe");
    hub.publish(vec![
        status("a", ServiceState::Running, Some(1), None),
        status("b", ServiceState::Running, Some(2), None),
    ]);
    let WaitOutcome::Snapshot { gen, services } = sub.wait_timeout(0, Duration::from_secs(1))
    else {
        panic!("expected snapshot");
    };
    assert_eq!(services.len(), 2);
    hub.publish(vec![status("a", ServiceState::Running, Some(1), None)]);
    match sub.wait_timeout(gen, Duration::from_secs(1)) {
        WaitOutcome::Snapshot { services, .. } => {
            assert_eq!(services.len(), 1);
            assert_eq!(services[0].name, "a");
        }
        WaitOutcome::Timeout => panic!("expected removal snapshot"),
    }
}
