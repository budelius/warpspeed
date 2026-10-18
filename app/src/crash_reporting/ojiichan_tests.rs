use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use sentry::protocol::{Attachment, Breadcrumb, User};
use serde_json::json;
use warp_errors::{REPORT_ERROR_LOCATION, report_error};

use super::*;

#[test]
fn reporting_keeps_code_locations_but_drops_sensitive_context() {
    let frame = Frame {
        function: Some("warp::workspace::render".into()),
        filename: Some(r"C:\Users\private\project\view.rs".into()),
        abs_path: Some("/home/private/project/view.rs".into()),
        lineno: Some(42),
        vars: [("token".into(), json!("secret"))].into(),
        context_line: Some("terminal output".into()),
        ..Default::default()
    };
    let event = Event {
        message: Some("command with secret".into()),
        release: Some("abc123".into()),
        server_name: Some("private-machine".into()),
        user: Some(User {
            email: Some("private@example.com".into()),
            ..Default::default()
        }),
        breadcrumbs: vec![Breadcrumb {
            message: Some("terminal content".into()),
            ..Default::default()
        }]
        .into(),
        extra: [("authorization".into(), json!("secret"))].into(),
        tags: [("cwd".into(), "/home/private/project".into())].into(),
        exception: vec![Exception {
            ty: "panic".into(),
            value: Some("secret panic payload".into()),
            stacktrace: Some(Stacktrace {
                frames: vec![frame],
                ..Default::default()
            }),
            ..Default::default()
        }]
        .into(),
        ..Default::default()
    };

    let sanitized = sanitize(event);
    let json = serde_json::to_string(&sanitized).unwrap();
    assert!(!json.contains("secret"));
    assert!(!json.contains("private"));
    assert!(!json.contains("terminal"));
    assert!(sanitized.breadcrumbs.is_empty());
    assert!(sanitized.user.is_none());
    assert!(sanitized.extra.is_empty());
    assert!(sanitized.tags.is_empty());
    assert_eq!(sanitized.release.as_deref(), Some("abc123"));
    let frame = &sanitized.exception[0].stacktrace.as_ref().unwrap().frames[0];
    assert_eq!(frame.filename.as_deref(), Some("view.rs"));
    assert_eq!(frame.function.as_deref(), Some("warp::workspace::render"));
    assert_eq!(frame.lineno, Some(42));
}

#[test]
fn panic_reports_identify_the_source_without_the_payload() {
    let filename = r"C:\Users\private\project\ansi_handler.rs";
    let event = sanitize(Event {
        exception: vec![Exception {
            ty: "panic".into(),
            value: Some("private panic payload".into()),
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    function: Some("rust_panic_location".into()),
                    filename: Some(filename.into()),
                    lineno: Some(42),
                    colno: Some(8),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }]
        .into(),
        ..Default::default()
    });

    assert_eq!(
        event.exception[0].value.as_deref(),
        Some("ansi_handler.rs:42:8")
    );
    assert!(!serde_json::to_string(&event).unwrap().contains("private"));
}

#[derive(Default)]
struct RecordedEnvelopes(Mutex<Vec<Envelope>>);

impl Transport for RecordedEnvelopes {
    fn send_envelope(&self, envelope: Envelope) {
        self.0.lock().unwrap().push(envelope);
    }
}

#[test]
fn non_panic_reports_identify_the_call_site_without_the_payload() {
    let event = sanitize(Event {
        exception: vec![Exception {
            ty: "private error type".into(),
            value: Some("private error payload".into()),
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    function: Some(REPORT_ERROR_LOCATION.into()),
                    filename: Some(r"C:\Users\private\project\listener.rs".into()),
                    lineno: Some(42),
                    colno: Some(8),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }]
        .into(),
        ..Default::default()
    });

    assert_eq!(event.exception[0].ty, "Error");
    assert_eq!(
        event.exception[0].value.as_deref(),
        Some("cloud-objects:listener.rs:42:8")
    );
    assert!(!serde_json::to_string(&event).unwrap().contains("private"));
}

#[test]
fn report_error_macro_preserves_a_sanitized_call_site() {
    let recorded = Arc::new(RecordedEnvelopes::default());
    let transport = Arc::new(DiagnosticsTransport(recorded.clone()));
    let mut options = ClientOptions {
        dsn: Some("https://public@example.invalid/1".parse().unwrap()),
        release: Some("test-build".into()),
        environment: Some("test".into()),
        ..Default::default()
    };
    configure(&mut options);
    options.transport = Some(Arc::new(transport));
    let client = Arc::new(sentry::Client::from(sentry::apply_defaults(options)));
    let hub = Arc::new(sentry::Hub::new(Some(client), Default::default()));

    let report_line = line!() + 2;
    sentry::Hub::run(hub, || {
        report_error!("synthetic crash-reporting test diagnostic");
    });

    let recorded = recorded.0.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let event = recorded[0].event().unwrap();
    assert_eq!(event.environment.as_deref(), Some("test"));
    assert_eq!(event.exception[0].ty, "Error");
    assert!(
        event.exception[0]
            .value
            .as_deref()
            .unwrap()
            .starts_with(&format!("application:ojiichan_tests.rs:{report_line}:"))
    );
    let frame = event.exception[0]
        .stacktrace
        .as_ref()
        .unwrap()
        .frames
        .iter()
        .find(|frame| frame.function.as_deref() == Some(REPORT_ERROR_LOCATION))
        .unwrap();
    assert_eq!(frame.filename.as_deref(), Some("ojiichan_tests.rs"));
    let json = serde_json::to_string(event).unwrap();
    assert!(!json.contains("synthetic crash-reporting test diagnostic"));
}

#[test]
fn panic_hook_reports_a_source_location_without_uploading_the_payload() {
    let recorded = Arc::new(RecordedEnvelopes::default());
    let transport = Arc::new(DiagnosticsTransport(recorded.clone()));
    let mut options = ClientOptions {
        dsn: Some("https://public@example.invalid/1".parse().unwrap()),
        release: Some("test-build".into()),
        ..Default::default()
    };
    configure(&mut options);
    options.transport = Some(Arc::new(transport));
    let client = Arc::new(sentry::Client::from(sentry::apply_defaults(options)));
    let hub = Arc::new(sentry::Hub::new(Some(client), Default::default()));

    let result = sentry::Hub::run(hub, || {
        std::panic::catch_unwind(|| panic!("private panic payload"))
    });

    assert!(result.is_err());
    let recorded = recorded.0.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let event = recorded[0].event().unwrap();
    assert_eq!(event.level, Level::Fatal);
    assert_eq!(event.release.as_deref(), Some("test-build"));
    assert_eq!(event.exception[0].ty, "panic");
    assert!(
        event.exception[0]
            .value
            .as_deref()
            .unwrap()
            .starts_with("ojiichan_tests.rs:")
    );
    let frame = event.exception[0]
        .stacktrace
        .as_ref()
        .unwrap()
        .frames
        .last()
        .unwrap();
    assert_eq!(frame.filename.as_deref(), Some("ojiichan_tests.rs"));
    assert!(frame.lineno.unwrap() > 0);
    assert!(frame.colno.unwrap() > 0);
    let json = serde_json::to_string(event).unwrap();
    assert!(!json.contains("private panic payload"));
    assert!(!json.contains("warp-crash-diagnostics"));
}

#[test]
fn missing_panic_locations_do_not_fall_back_to_the_payload() {
    let event = sanitize(Event {
        exception: vec![Exception {
            ty: "panic".into(),
            value: Some("private panic payload".into()),
            ..Default::default()
        }]
        .into(),
        ..Default::default()
    });

    assert_eq!(event.exception[0].ty, "panic");
    assert!(event.exception[0].value.is_none());
}

#[test]
fn transport_never_uploads_attachments() {
    let recorded = Arc::new(RecordedEnvelopes::default());
    let transport = DiagnosticsTransport(recorded.clone());
    let mut envelope = Envelope::from(sanitize(Event::default()));
    envelope.add_item(Attachment {
        buffer: b"private terminal contents".to_vec(),
        filename: "memory.dmp".into(),
        ..Default::default()
    });

    transport.send_envelope(envelope);

    let recorded = recorded.0.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].items().count(), 1);
    let mut bytes = Vec::new();
    recorded[0].to_writer(&mut bytes).unwrap();
    let body = String::from_utf8(bytes).unwrap();
    assert!(!body.contains("private terminal contents"));
    assert!(!body.contains("memory.dmp"));
}

#[test]
fn debug_derived_exception_names_are_not_uploaded() {
    let event = sanitize(Event {
        exception: vec![Exception {
            ty: "secret-server-response".into(),
            ..Default::default()
        }]
        .into(),
        ..Default::default()
    });
    assert_eq!(event.exception[0].ty, "Error");
}

#[test]
fn ojiichan_preserves_only_bounded_supervisor_metadata() {
    let event = sanitize(Event {
        message: Some("Terminal session supervisor exited unexpectedly".into()),
        environment: Some("private-environment".into()),
        platform: "private-platform".into(),
        tags: [
            (
                HEADLESS_PROCESS_TAG.into(),
                "terminal_session_supervisor".into(),
            ),
            (
                HEADLESS_EXIT_REASON_TAG.into(),
                "control_channel_closed".into(),
            ),
            (HEADLESS_HAD_LIVE_SESSIONS_TAG.into(), "yes".into()),
            ("cwd".into(), "/home/private/project".into()),
        ]
        .into(),
        exception: vec![Exception {
            ty: "panic".into(),
            value: Some("private panic".into()),
            ..Default::default()
        }]
        .into(),
        stacktrace: Some(Stacktrace {
            frames: vec![Frame {
                filename: Some("private.rs".into()),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    });

    assert_eq!(
        event.message.as_deref(),
        Some("Terminal session supervisor exited unexpectedly")
    );
    assert_eq!(event.tags.len(), 3);
    assert!(event.environment.is_none());
    assert!(event.platform.is_empty());
    assert!(event.exception.is_empty());
    assert!(event.stacktrace.is_none());
    assert!(!serde_json::to_string(&event).unwrap().contains("private"));

    let invalid = sanitize(Event {
        tags: [(HEADLESS_PROCESS_TAG.into(), "private-process-name".into())].into(),
        ..Default::default()
    });
    assert!(invalid.tags.is_empty());
}

#[test]
fn ojiichan_composes_with_existing_event_filters() {
    let called = Arc::new(AtomicBool::new(false));
    let called_by_filter = called.clone();
    let mut options = ClientOptions {
        before_send: Some(Arc::new(move |_| {
            called_by_filter.store(true, Ordering::Release);
            None
        })),
        ..Default::default()
    };

    configure(&mut options);
    assert!(options.before_send.unwrap()(Event::default()).is_none());
    assert!(called.load(Ordering::Acquire));
}

#[test]
fn interrupted_session_reports_remain_identifiable() {
    let event = sanitize(Event {
        message: Some("WarpSpeed interrupted session recovered".into()),
        release: Some("crashed-build".into()),
        ..Default::default()
    });
    assert_eq!(
        event.message.as_deref(),
        Some("WarpSpeed interrupted session recovered")
    );
    assert_eq!(event.release.as_deref(), Some("crashed-build"));
}
