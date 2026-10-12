use std::io::{Cursor, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use super::*;
use crate::{HttpStatusError, STAGING_FLEET_ID, STAGING_ZITADEL};

const EVENT: &str = r#"{"event_id":"event-3","job_id":"job-test","sequence":3,"timestamp":"now","type":"assistant","content":"héllo","redacted":false,"truncated":false}"#;
const PAGE: &str =
    r#"{"items":[],"next_cursor":"cursor-3","terminal":true,"gap":false,"earliest_cursor":null}"#;

fn fixture(
    responses: Vec<(u16, &'static str, String)>,
) -> (EnvironmentProfile, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = Box::leak(format!("http://{}", listener.local_addr().unwrap()).into_boxed_str());
    let (sender, requests) = mpsc::channel();
    thread::spawn(move || {
        for (status, content_type, body) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = String::new();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            sender.send(request).unwrap();
            write!(socket, "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (
        EnvironmentProfile {
            alias: "staging",
            base_url,
            fleet_id: STAGING_FLEET_ID,
            zitadel: &STAGING_ZITADEL,
        },
        requests,
    )
}

#[test]
fn streams_authenticated_events_from_last_applied_cursor() {
    let (profile, requests) = fixture(vec![(
        200,
        "text/event-stream",
        format!(
            "id: cursor-3\r\nevent: run_event\r\ndata: {EVENT}\r\n\r\nevent: terminal\ndata: {{}}\n\n"
        ),
    )]);
    let mut feed = EventFeed::new().unwrap();
    let (page, gap) = feed
        .next(&profile, "job-test", "test-token", Some("cursor-2"), 0)
        .unwrap();
    assert_eq!(page.items[0].content, "héllo");
    assert_eq!(page.next_cursor.as_deref(), Some("cursor-3"));
    assert!(!gap);
    assert!(
        feed.next(&profile, "job-test", "test-token", Some("cursor-3"), 0)
            .unwrap()
            .0
            .terminal
    );
    let request = requests.recv().unwrap().to_lowercase();
    assert!(request.starts_with("get /api/agent-control/runs/job-test/events/stream "));
    assert!(request.contains("authorization: bearer test-token"));
    assert!(request.contains("last-event-id: cursor-2"));
    assert!(request.contains("x-stozn-client: warpspeed"));
}

#[test]
fn gap_resumes_before_the_first_retained_event() {
    let (profile, requests) = fixture(vec![
        (
            200,
            "text/event-stream",
            "event: gap\ndata: {\"earliest_cursor\":\"cursor-2\"}\n\n".into(),
        ),
        (
            200,
            "text/event-stream",
            format!("id: cursor-3\nevent: run_event\ndata: {EVENT}\n\n"),
        ),
    ]);
    let mut feed = EventFeed::new().unwrap();
    let page = feed.next(&profile, "job-test", "token", None, 0).unwrap().0;
    assert!(page.gap);
    assert_eq!(page.next_cursor.as_deref(), Some("cursor-2"));
    let next = feed
        .next(
            &profile,
            "job-test",
            "token",
            page.next_cursor.as_deref(),
            0,
        )
        .unwrap()
        .0;
    assert_eq!(next.items[0].sequence, 3);
    requests.recv().unwrap();
    assert!(requests.recv().unwrap().contains("last-event-id: cursor-2"));
}

#[test]
fn older_endpoint_falls_back_to_polling() {
    let (profile, requests) = fixture(vec![
        (404, "application/json", "{}".into()),
        (200, "application/json", PAGE.into()),
        (200, "application/json", PAGE.into()),
    ]);
    let mut feed = EventFeed::new().unwrap();
    assert!(
        feed.next(&profile, "job-test", "token", Some("cursor-3"), 0)
            .unwrap()
            .0
            .terminal
    );
    assert!(
        feed.next(&profile, "job-test", "token", Some("cursor-3"), 0)
            .unwrap()
            .0
            .terminal
    );
    requests.recv().unwrap();
    assert!(
        requests
            .recv()
            .unwrap()
            .starts_with("GET /api/agent-control/runs/job-test/events?")
    );
    assert!(
        requests
            .recv()
            .unwrap()
            .starts_with("GET /api/agent-control/runs/job-test/events?")
    );
}

#[test]
fn stream_failure_preserves_cursor_for_polling() {
    let (profile, requests) = fixture(vec![
        (
            200,
            "text/event-stream",
            "event: error\ndata: {\"code\":\"storage_unavailable\"}\n\n".into(),
        ),
        (200, "application/json", PAGE.into()),
    ]);
    let mut feed = EventFeed::new().unwrap();
    assert!(
        feed.next(&profile, "job-test", "token", Some("cursor-3"), 0)
            .unwrap()
            .0
            .terminal
    );
    requests.recv().unwrap();
    assert!(requests.recv().unwrap().contains("after=cursor-3"));
}

#[test]
fn reauth_reopens_with_a_new_bearer_and_the_applied_cursor() {
    let (profile, requests) = fixture(vec![
        (
            200,
            "text/event-stream",
            "event: reauth\ndata: {}\n\n".into(),
        ),
        (
            200,
            "text/event-stream",
            "event: terminal\ndata: {}\n\n".into(),
        ),
    ]);
    let mut feed = EventFeed::new().unwrap();
    let page = feed
        .next(&profile, "job-test", "old", Some("cursor-3"), 0)
        .unwrap()
        .0;
    assert!(!page.terminal);
    assert_eq!(page.next_cursor.as_deref(), Some("cursor-3"));
    assert!(
        feed.next(&profile, "job-test", "new", Some("cursor-3"), 0)
            .unwrap()
            .0
            .terminal
    );
    requests.recv().unwrap();
    let request = requests.recv().unwrap();
    assert!(request.contains("authorization: Bearer new"));
    assert!(request.contains("last-event-id: cursor-3"));
}

#[test]
fn unauthorized_is_not_hidden_by_fallback() {
    let (profile, requests) = fixture(vec![(401, "application/json", "{}".into())]);
    let error = EventFeed::new()
        .unwrap()
        .next(&profile, "job-test", "token", None, 0)
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<HttpStatusError>().unwrap().status,
        StatusCode::UNAUTHORIZED
    );
    requests.recv().unwrap();
}

#[test]
fn multiline_data_is_decoded_without_advancing_unknown_events() {
    let mut data = Cursor::new(format!(
        "event: run_event\nid: opaque\ndata: {{\ndata: {}\n\n",
        &EVENT[1..]
    ));
    let page = read_frame(&mut data).unwrap().page(None).unwrap().unwrap();
    assert_eq!(page.items[0].content, "héllo");
    let page = read_frame(&mut Cursor::new("id: ignored\nevent: future\ndata: {}\n\n"))
        .unwrap()
        .page(Some("applied"))
        .unwrap()
        .unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("applied"));
}

#[test]
fn polling_fallback_drains_output_before_terminal() {
    let page = format!(
        r#"{{"items":[{EVENT}],"next_cursor":"cursor-3","terminal":true,"gap":false,"earliest_cursor":null}}"#
    );
    let (profile, requests) = fixture(vec![
        (405, "application/json", "{}".into()),
        (200, "application/json", page),
        (200, "application/json", PAGE.into()),
    ]);
    let mut feed = EventFeed::new().unwrap();
    let page = feed.next(&profile, "job-test", "token", None, 0).unwrap().0;
    assert!(!page.terminal);
    assert_eq!(page.items.len(), 1);
    assert!(
        feed.next(
            &profile,
            "job-test",
            "token",
            page.next_cursor.as_deref(),
            0
        )
        .unwrap()
        .0
        .terminal
    );
    requests.recv().unwrap();
    requests.recv().unwrap();
    assert!(requests.recv().unwrap().contains("after=cursor-3"));
}

#[test]
fn keepalive_preserves_cursor() {
    let page = read_frame(&mut Cursor::new(": keepalive\n\n"))
        .unwrap()
        .page(Some("applied"))
        .unwrap()
        .unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("applied"));
    assert!(page.items.is_empty());
}

#[test]
fn incomplete_and_oversized_frames_are_rejected() {
    assert!(read_frame(&mut Cursor::new("event: terminal\ndata: {}\n")).is_err());
    assert!(read_frame(&mut Cursor::new(vec![b'x'; MAX_FRAME_BYTES as usize + 1])).is_err());
}
