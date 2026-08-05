//! Unit/integration tests for microinit::protocol

use microinit::protocol::*;

#[test]
fn service_state_display() {
    assert_eq!(ServiceState::Running.to_string(), "running");
    assert_eq!(ServiceState::Disabled.to_string(), "disabled");
    assert_eq!(
        ServiceState::WaitingForDependency.to_string(),
        "waiting_for_dependency"
    );
}

#[test]
fn request_response_serde_roundtrip() {
    let cases = vec![
        Request::List,
        Request::Start {
            name: "a".into(),
            force: false,
        },
        Request::Enable {
            name: "b".into(),
            enabled: false,
        },
        Request::Logs {
            name: None,
            follow: true,
            lines: Some(50),
        },
        Request::Shutdown {
            mode: ShutdownMode::Poweroff,
        },
        Request::Describe {
            name: "nginx".into(),
        },
    ];
    for req in cases {
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(json, json2);
    }

    let resp = Response::Status {
        status: ServiceStatus {
            name: "redis".into(),
            state: ServiceState::Failed,
            pid: None,
            restarts: 2,
            liveness_failures: 1,
            enabled: true,
        },
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"type\":\"status\""));
    assert!(json.contains("\"state\":\"failed\""));
    let _: Response = serde_json::from_str(&json).unwrap();

    let describe = Response::Describe {
        describe: ServiceDescribe {
            status: ServiceStatus {
                name: "nginx".into(),
                state: ServiceState::Running,
                pid: Some(1),
                restarts: 0,
                liveness_failures: 0,
                enabled: true,
            },
            uptime_secs: Some(10),
            depends_on: vec![DepNode {
                name: "php-fpm".into(),
                state: ServiceState::Running,
            }],
            dependents: vec![],
            dep_nodes: vec![
                DepNode {
                    name: "nginx".into(),
                    state: ServiceState::Running,
                },
                DepNode {
                    name: "php-fpm".into(),
                    state: ServiceState::Running,
                },
            ],
            dep_edges: vec![("php-fpm".into(), "nginx".into())],
            events: vec![ServiceEvent {
                ts: "2026-08-04T18:40:01.123Z".into(),
                kind: ServiceEventKind::StateChange,
                from: Some(ServiceState::Pending),
                to: Some(ServiceState::Starting),
                detail: None,
            }],
        },
    };
    let json = serde_json::to_string(&describe).unwrap();
    assert!(json.contains("\"type\":\"describe\""));
    assert!(json.contains("\"kind\":\"state_change\""));
    let _: Response = serde_json::from_str(&json).unwrap();
}

#[test]
fn describe_event_kinds_serde_roundtrip() {
    let events = vec![
        ServiceEvent {
            ts: "2026-08-04T18:40:01.123Z".into(),
            kind: ServiceEventKind::StateChange,
            from: Some(ServiceState::Pending),
            to: Some(ServiceState::Starting),
            detail: None,
        },
        ServiceEvent {
            ts: "2026-08-04T18:51:10.001Z".into(),
            kind: ServiceEventKind::LivenessFailed,
            from: None,
            to: None,
            detail: Some("HTTP 503".into()),
        },
        ServiceEvent {
            ts: "2026-08-04T18:51:10.002Z".into(),
            kind: ServiceEventKind::Restart,
            from: None,
            to: None,
            detail: None,
        },
    ];

    for ev in &events {
        let json = serde_json::to_string(ev).unwrap();
        let back: ServiceEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev.kind, back.kind);
        assert_eq!(ev.from, back.from);
        assert_eq!(ev.to, back.to);
        assert_eq!(ev.detail, back.detail);
        assert_eq!(ev.ts, back.ts);
    }

    let restart_json = serde_json::to_string(&events[2]).unwrap();
    assert!(restart_json.contains("\"kind\":\"restart\""));
    assert!(!restart_json.contains("\"from\""));
    assert!(!restart_json.contains("\"detail\""));

    let fail_json = serde_json::to_string(&events[1]).unwrap();
    assert!(fail_json.contains("\"kind\":\"liveness_failed\""));
    assert!(fail_json.contains("\"detail\":\"HTTP 503\""));

    // Empty deps still round-trip.
    let empty = Response::Describe {
        describe: ServiceDescribe {
            status: ServiceStatus {
                name: "solo".into(),
                state: ServiceState::Stopped,
                pid: None,
                restarts: 0,
                liveness_failures: 0,
                enabled: true,
            },
            uptime_secs: None,
            depends_on: vec![],
            dependents: vec![],
            dep_nodes: vec![DepNode {
                name: "solo".into(),
                state: ServiceState::Stopped,
            }],
            dep_edges: vec![],
            events: events.clone(),
        },
    };
    let json = serde_json::to_string(&empty).unwrap();
    let _: Response = serde_json::from_str(&json).unwrap();
}

#[test]
fn request_tagged_type_field() {
    let v: serde_json::Value = serde_json::from_str(r#"{"type":"start","name":"redis"}"#).unwrap();
    let req: Request = serde_json::from_value(v).unwrap();
    assert!(matches!(req, Request::Start { name, force: false } if name == "redis"));
}
