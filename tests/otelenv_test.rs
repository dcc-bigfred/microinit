use microinit::config::OpenTelemetryConfig;
use microinit::otelenv;
use serial_test::serial;

#[test]
fn parse_skips_comments() {
    let m = otelenv::parse("# c\nENABLE_TELEMETRY=true\n\nOTEL_SERVICE_NAME=\"microinit\"\nbad\n");
    assert_eq!(m.get("ENABLE_TELEMETRY").map(String::as_str), Some("true"));
    assert_eq!(
        m.get("OTEL_SERVICE_NAME").map(String::as_str),
        Some("microinit")
    );
}

#[test]
#[serial]
fn normalize_maps_grpc_port_to_http() {
    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "127.0.0.1:4317");
    let cfg = otelenv::overlay_config(OpenTelemetryConfig::default());
    assert_eq!(cfg.endpoint, "http://127.0.0.1:4318");
    std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");

    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318");
    let cfg = otelenv::overlay_config(OpenTelemetryConfig::default());
    assert_eq!(cfg.endpoint, "http://127.0.0.1:4318");
    std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");

    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "alloy:4318");
    let cfg = otelenv::overlay_config(OpenTelemetryConfig::default());
    assert_eq!(cfg.endpoint, "http://alloy:4318");
    std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
}

#[test]
#[serial]
fn overlay_enable_and_interval() {
    std::env::set_var("ENABLE_TELEMETRY", "true");
    std::env::set_var("OTEL_METRIC_EXPORT_INTERVAL", "30000");
    std::env::set_var("OTEL_SERVICE_NAME", "mi-test");
    let cfg = otelenv::overlay_config(OpenTelemetryConfig::default());
    assert!(cfg.enable);
    assert_eq!(cfg.export_interval_secs, 30);
    assert_eq!(cfg.service_name, "mi-test");
    std::env::remove_var("ENABLE_TELEMETRY");
    std::env::remove_var("OTEL_METRIC_EXPORT_INTERVAL");
    std::env::remove_var("OTEL_SERVICE_NAME");
}

#[test]
#[serial]
fn parse_headers_pairs() {
    std::env::set_var(
        "OTEL_EXPORTER_OTLP_HEADERS",
        "Authorization=Bearer x, X-Scope=a",
    );
    let cfg = otelenv::overlay_config(OpenTelemetryConfig::default());
    assert_eq!(
        cfg.headers.get("Authorization").map(String::as_str),
        Some("Bearer x")
    );
    assert_eq!(cfg.headers.get("X-Scope").map(String::as_str), Some("a"));
    std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS");
}
