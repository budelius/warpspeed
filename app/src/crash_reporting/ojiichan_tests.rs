use std::sync::Mutex;

use sentry::protocol::{Attachment, Breadcrumb, User};
use serde_json::json;

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

#[derive(Default)]
struct RecordedEnvelopes(Mutex<Vec<Envelope>>);

impl Transport for RecordedEnvelopes {
    fn send_envelope(&self, envelope: Envelope) {
        self.0.lock().unwrap().push(envelope);
    }
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
