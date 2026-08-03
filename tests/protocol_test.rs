//! Unit/integration tests for microinit::protocol

use microinit::protocol::*;

#[test]
fn service_state_display() {
    assert_eq!(ServiceState::Running.to_string(), "running");
    assert_eq!(ServiceState::Disabled.to_string(), "disabled");
}

#[test]
fn request_response_serde_roundtrip() {
    let cases = vec![
        Request::List,
        Request::Start { name: "a".into() },
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
            enabled: true,
        },
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"type\":\"status\""));
    assert!(json.contains("\"state\":\"failed\""));
    let _: Response = serde_json::from_str(&json).unwrap();
}

#[test]
fn request_tagged_type_field() {
    let v: serde_json::Value = serde_json::from_str(r#"{"type":"start","name":"redis"}"#).unwrap();
    let req: Request = serde_json::from_value(v).unwrap();
    assert!(matches!(req, Request::Start { name } if name == "redis"));
}
