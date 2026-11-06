use super::*;
use crate::{RunEvent, RunPreviewEventKind};

fn event(sequence: u64) -> RunEvent {
    RunEvent {
        event_id: format!("event-{sequence}"),
        job_id: "job-preview".into(),
        sequence,
        timestamp: "now".into(),
        event_type: "assistant".into(),
        content: format!("message {sequence}\u{1b}"),
        redacted: false,
        truncated: false,
    }
}

fn page(items: Vec<RunEvent>, terminal: bool) -> EventPage {
    EventPage {
        items,
        next_cursor: Some("cursor".into()),
        terminal,
        gap: false,
        earliest_cursor: None,
    }
}

#[test]
fn preview_stream_keeps_a_bounded_sanitized_tail() {
    let mut stream = RunPreviewStream::new(ManagedEnvironment::Staging, "job-preview").unwrap();
    stream.apply(page((1..=20).map(event).collect(), false), false);
    assert_eq!(stream.preview.events.len(), 12);
    assert_eq!(stream.preview.events[0].content, "message 9�");
    assert_eq!(stream.preview.events[11].content, "message 20�");
    assert_eq!(
        stream.preview.events[0].kind,
        RunPreviewEventKind::Assistant
    );
    assert!(stream.preview.truncated);
}

#[test]
fn replay_does_not_duplicate_preview_output() {
    let mut stream = RunPreviewStream::new(ManagedEnvironment::Staging, "job-preview").unwrap();
    stream.apply(page(vec![event(1)], false), false);
    stream.apply(page(vec![event(1), event(2)], false), false);
    assert_eq!(stream.preview.events.len(), 2);
    assert_eq!(stream.preview.events[1].content, "message 2�");
}

#[test]
fn terminal_polling_pages_are_drained_before_finishing() {
    let mut stream = RunPreviewStream::new(ManagedEnvironment::Staging, "job-preview").unwrap();
    stream.apply(page(vec![event(1)], true), true);
    assert!(!stream.preview.terminal);
    assert!(stream.preview.history_gap);
    stream.apply(page(Vec::new(), true), false);
    assert!(stream.preview.terminal);
    assert!(stream.preview.history_gap);
    assert_eq!(stream.preview.events.len(), 1);
}
