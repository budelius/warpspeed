use super::CrashReportingConfig;

#[test]
fn channel_debug_logging_does_not_expose_the_reporting_key() {
    let config = CrashReportingConfig {
        sentry_url: "https://private-key@ingest.example.test/42".into(),
    };
    let debug = format!("{config:?}");
    assert!(!debug.contains("private-key"));
    assert!(!debug.contains("ingest.example.test"));
    assert!(debug.contains("[redacted]"));
}
