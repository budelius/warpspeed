use std::io::Read as _;
use std::net::TcpListener;

use super::*;

#[test]
fn dashboard_deep_link_opens_only_the_fixed_managed_session() {
    let job_id = "job-ca9b590c-979c-4d46-b19f-f6f50fc852d3";
    let url = reqwest::Url::parse(&format!(
        "warpspeed://agent-ctrl/attach?environment=staging&fleet_id={STAGING_FLEET_ID}&job_id={job_id}"
    ))
    .unwrap();
    let intent = parse_managed_session_deep_link(&url).unwrap();
    assert_eq!(intent.environment, ManagedEnvironment::Staging);
    assert_eq!(intent.job_id, job_id);
    assert_eq!(
        intent.cli_arguments(),
        format!("agent-ctrl open {job_id} --environment staging")
    );
}

#[test]
fn dashboard_deep_link_rejects_redirects_overrides_and_shell_content() {
    let invalid = [
        format!(
            "https://agent-ctrl/attach?environment=staging&fleet_id={STAGING_FLEET_ID}&job_id=job-safe"
        ),
        "warpspeed://attacker.invalid/attach?environment=staging&fleet_id=stoz3n-agent-ctrl-staging&job_id=job-safe".to_owned(),
        "warpspeed://agent-ctrl/other?environment=staging&fleet_id=stoz3n-agent-ctrl-staging&job_id=job-safe".to_owned(),
        "warpspeed://agent-ctrl/attach?environment=production&fleet_id=stoz3n-agent-ctrl-staging&job_id=job-safe".to_owned(),
        "warpspeed://agent-ctrl/attach?environment=staging&fleet_id=other&job_id=job-safe".to_owned(),
        "warpspeed://agent-ctrl/attach?environment=staging&fleet_id=stoz3n-agent-ctrl-staging&job_id=job-safe&job_id=job-other".to_owned(),
        "warpspeed://agent-ctrl/attach?environment=staging&fleet_id=stoz3n-agent-ctrl-staging&job_id=job-safe%26calc.exe".to_owned(),
        "warpspeed://agent-ctrl/attach?environment=staging&fleet_id=stoz3n-agent-ctrl-staging&job_id=job-safe&controller=https%3A%2F%2Fattacker.invalid".to_owned(),
    ];
    for value in invalid {
        let url = reqwest::Url::parse(&value).unwrap();
        assert!(
            parse_managed_session_deep_link(&url).is_err(),
            "unexpectedly accepted {value}"
        );
    }
}

#[test]
fn stozn_login_url_is_bound_to_the_compiled_dashboard_and_request() {
    let profile = profile(ManagedEnvironment::Staging);
    let request_id = "a".repeat(43);
    let valid = format!("{}/warpspeed-auth?request_id={request_id}", profile.web_url);
    assert!(validate_verification_url(profile, &valid, &request_id).is_ok());
    assert!(
        validate_verification_url(
            profile,
            &format!("https://attacker.invalid/warpspeed-auth?request_id={request_id}"),
            &request_id,
        )
        .is_err()
    );
    assert!(
        validate_verification_url(
            profile,
            &format!(
                "{}/warpspeed-auth?request_id={request_id}",
                profile.base_url
            ),
            &request_id,
        )
        .is_err()
    );
    assert!(
        validate_verification_url(
            profile,
            &format!("{}/warpspeed-auth?request_id=wrong", profile.web_url),
            &request_id,
        )
        .is_err()
    );
}

#[test]
fn stozn_confirmation_codes_are_strictly_formatted() {
    assert!(valid_user_code("A1B2-C3D4"));
    assert!(!valid_user_code("a1b2-c3d4"));
    assert!(!valid_user_code("A1B2C3D4"));
    assert!(!valid_user_code("A1B2-G3D4"));
}

#[test]
fn stozn_desktop_credential_protection_round_trips_for_this_os_user() {
    let plaintext = br#"{\"token\":\"sensitive-test-value\"}"#.to_vec();
    let protected = protect_auth_bytes(plaintext.clone()).unwrap();
    #[cfg(windows)]
    assert_ne!(protected, plaintext);
    assert_eq!(unprotect_auth_bytes(protected).unwrap(), plaintext);
}

#[test]
fn stozn_login_state_requires_admin_and_visible_ascii_credentials() {
    assert!(valid_dashboard_token("header.payload.signature"));
    assert!(!valid_dashboard_token(""));
    assert!(!valid_dashboard_token("token with spaces"));
    let identity: StoznIdentity = serde_json::from_value(json!({
        "sub": "@operator:stozn.ai",
        "role": "admin",
        "display_name": "Operator"
    }))
    .unwrap();
    assert_eq!(identity.subject, "@operator:stozn.ai");
    assert_eq!(identity.role, "admin");
}

#[test]
fn waiting_input_remains_controllable_in_the_ui() {
    assert!(!session_is_finished("waiting_input"));
    assert!(!session_is_finished("running"));
    assert!(session_is_finished("completed"));
    assert!(session_is_finished("interrupted"));
}

fn http_fixture(status: &str, body: serde_json::Value) -> &'static str {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let body = body.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 8192];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        stream.flush().unwrap();
    });
    Box::leak(format!("http://{address}").into_boxed_str())
}

fn fixture_profile(base_url: &'static str) -> EnvironmentProfile {
    EnvironmentProfile {
        alias: "staging",
        base_url,
        web_url: STAGING_WEB_URL,
        fleet_id: STAGING_FLEET_ID,
    }
}

fn descriptor_json() -> serde_json::Value {
    json!({
        "protocol_version": 1,
        "environment": "staging",
        "fleet_id": STAGING_FLEET_ID,
        "job_id": "job-123",
        "display_name": "Inspector",
        "agent_name": "staging-pi-inspector",
        "state": "running",
        "capabilities": ["watch", "input", "close"],
        "next_cursor": null
    })
}

#[test]
fn bookmark_serialization_contains_only_safe_fields() {
    let bookmark = ManagedSessionBookmark {
        format_version: 1,
        environment_alias: "staging".to_owned(),
        fleet_id: "fleet".to_owned(),
        job_id: "job-123".to_owned(),
        last_cursor: Some("cursor".to_owned()),
        display_name: Some("inspector".to_owned()),
        agent_name: Some("staging-pi-inspector".to_owned()),
        last_known_state: Some("running".to_owned()),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        last_opened_at: "2026-01-01T00:00:01Z".to_owned(),
    };
    let value = serde_json::to_value(bookmark).unwrap();
    let object = value.as_object().unwrap();
    let expected = [
        "format_version",
        "environment_alias",
        "fleet_id",
        "job_id",
        "last_cursor",
        "display_name",
        "agent_name",
        "last_known_state",
        "created_at",
        "last_opened_at",
    ];
    assert_eq!(object.len(), expected.len());
    for key in expected {
        assert!(object.contains_key(key), "missing safe key {key}");
    }
    for forbidden in [
        "token",
        "authorization",
        "prompt",
        "transcript",
        "events",
        "url",
        "endpoint",
        "controller",
        "environment_variables",
        "reasoning",
    ] {
        assert!(!object.contains_key(forbidden));
    }
}

#[test]
fn task_ids_are_bounded_before_launch() {
    assert!(validate_task_id("67d000000000000000000001").is_ok());
    assert!(validate_task_id("task:staging-42").is_ok());
    assert!(validate_task_id("").is_err());
    assert!(validate_task_id("task/../../secret").is_err());
}

#[test]
fn command_response_must_be_explicitly_accepted() {
    assert!(ensure_command_accepted(&json!({ "status": "accepted" })).is_ok());
    assert!(
        ensure_command_accepted(&json!({
            "status": "rejected",
            "error": { "error": "node unavailable" }
        }))
        .is_err()
    );
    assert!(ensure_command_accepted(&json!({})).is_err());
}

#[test]
fn bookmark_upsert_preserves_creation_and_resume_cursor() {
    let mut bookmarks = vec![ManagedSessionBookmark {
        format_version: 1,
        environment_alias: "staging".to_owned(),
        fleet_id: STAGING_FLEET_ID.to_owned(),
        job_id: "job-123".to_owned(),
        last_cursor: Some("old".to_owned()),
        display_name: None,
        agent_name: None,
        last_known_state: Some("running".to_owned()),
        created_at: "created".to_owned(),
        last_opened_at: "first".to_owned(),
    }];
    upsert_bookmark(
        &mut bookmarks,
        ManagedSessionBookmark {
            format_version: 1,
            environment_alias: "staging".to_owned(),
            fleet_id: STAGING_FLEET_ID.to_owned(),
            job_id: "job-123".to_owned(),
            last_cursor: Some("next".to_owned()),
            display_name: Some("run".to_owned()),
            agent_name: Some("agent".to_owned()),
            last_known_state: Some("running".to_owned()),
            created_at: "replacement".to_owned(),
            last_opened_at: "second".to_owned(),
        },
    );
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].created_at, "created");
    assert_eq!(bookmarks[0].last_cursor.as_deref(), Some("next"));
}

#[test]
fn descriptor_must_match_compiled_environment_identity() {
    let descriptor = SessionDescriptor {
        protocol_version: 1,
        environment: "staging".to_owned(),
        fleet_id: STAGING_FLEET_ID.to_owned(),
        job_id: "job-123".to_owned(),
        display_name: "run".to_owned(),
        agent_name: "agent".to_owned(),
        state: "running".to_owned(),
        capabilities: vec!["watch".to_owned()],
        next_cursor: None,
    };
    assert!(
        validate_descriptor(profile(ManagedEnvironment::Staging), "job-123", &descriptor).is_ok()
    );
    assert!(
        validate_descriptor(
            profile(ManagedEnvironment::Staging),
            "job-other",
            &descriptor
        )
        .is_err()
    );
}

#[test]
fn http_auth_and_expired_token_responses_fail_closed() {
    for (status, code) in [
        ("401 Unauthorized", "unauthenticated"),
        ("403 Forbidden", "forbidden"),
        ("401 Unauthorized", "token_expired"),
    ] {
        let base = http_fixture(status, json!({ "error": "denied", "code": code }));
        let profile = fixture_profile(base);
        let error = send(
            &profile,
            Client::new().get(format!("{base}/api/agent-control/runs")),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(status.split_once(' ').unwrap().0));
        assert!(error.contains(code));
    }
}

#[test]
fn http_gap_fixture_deduplicates_replayed_events() {
    let event = json!({
        "event_id": "event-3",
        "job_id": "job-123",
        "sequence": 3,
        "timestamp": "2026-01-01T00:00:00Z",
        "type": "assistant",
        "content": "safe output",
        "redacted": false,
        "truncated": false
    });
    let base = http_fixture(
        "410 Gone",
        json!({
            "items": [event.clone(), event],
            "next_cursor": "djE6Mw",
            "terminal": false,
            "gap": true,
            "earliest_cursor": "djE6Mw"
        }),
    );
    let profile = fixture_profile(base);
    let (page, history_gap) = fetch_event_page(
        &Client::new(),
        &profile,
        "job-123",
        "test-token",
        Some("djE6MA"),
        0,
    )
    .unwrap();
    assert!(history_gap);
    assert!(page.gap);
    let mut known = HashSet::new();
    let events = retain_new_events(page.items, &mut known);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "event-3");
}

#[test]
fn full_managed_flow_has_dashboard_only_network_authority() {
    let profile = profile(ManagedEnvironment::Staging);
    let client = Client::new();
    for (method, path) in [
        ("GET", "/api/agent-control/runs?limit=100"),
        ("GET", "/api/work-items/?open_only=true&limit=100"),
        ("GET", "/api/agent-control/overview"),
        ("POST", "/api/agent-control/runs"),
        ("GET", "/api/agent-control/runs/job-123/session"),
        ("GET", "/api/agent-control/runs/job-123/events"),
        ("POST", "/api/agent-control/runs/job-123/input"),
        ("POST", "/api/agent-control/runs/job-123/close"),
        ("POST", "/api/agent-control/runs/job-123/stop"),
    ] {
        let request = client.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{path}", profile.base_url),
        );
        validate_request_destination(profile, &request).unwrap();
    }

    for forbidden in [
        "https://api.warp.dev/managed-session",
        "https://app.warp.dev/api/session-sharing",
        "https://rtc.warp.dev/connect",
        "https://oz.warp.dev/run",
        "https://releases.warp.dev/artifacts/session",
        "https://app.warp.dev/api/v1/inference",
    ] {
        let error = send(profile, client.get(forbidden))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("destination is not the selected dashboard"),
            "{forbidden} was not denied before network I/O: {error}"
        );
    }
}

#[test]
fn http_descriptor_fixtures_reject_protocol_fleet_environment_and_job_mismatch() {
    for (field, value) in [
        ("protocol_version", json!(2)),
        ("environment", json!("production")),
        ("fleet_id", json!("other-fleet")),
        ("job_id", json!("job-other")),
    ] {
        let mut body = descriptor_json();
        body[field] = value;
        let base = http_fixture("200 OK", body);
        let profile = fixture_profile(base);
        assert!(
            fetch_descriptor(&Client::new(), &profile, "job-123", "test-token").is_err(),
            "{field} mismatch must fail closed"
        );
    }
}

fn capturing_fixture(
    body: serde_json::Value,
) -> (&'static str, std::sync::mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let body = body.to_string();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            request.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&request);
            if let Some(header_end) = text.find("\r\n\r\n") {
                let length = text[..header_end]
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + length || read == 0 {
                    break;
                }
            } else if read == 0 {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        stream.flush().unwrap();
        sender.send(request).unwrap();
    });
    (
        Box::leak(format!("http://{address}").into_boxed_str()),
        receiver,
    )
}

fn moved_import(session_file: Vec<u8>) -> MovedSessionImport<'static> {
    MovedSessionImport {
        agent_name: "staging-claude-moved",
        session_id: "abc-123",
        session_file,
        repo_url: "https://git.example.com/org/repo.git",
        handoff_branch: "handoff/abc-123",
        source_cwd: "/home/user/project",
        idempotency_key: "warpspeed-key-1".to_owned(),
    }
}

#[test]
fn import_sends_multipart_and_returns_job_id() {
    let (base, request) = capturing_fixture(json!({ "status": "accepted", "job_id": "job-1" }));
    let profile = fixture_profile(base);
    let job_id =
        import_moved_session_with(&profile, "token", moved_import(b"{\"a\":1}\n".to_vec()))
            .unwrap();
    assert_eq!(job_id, "job-1");
    let raw = String::from_utf8_lossy(&request.recv().unwrap()).into_owned();
    let lower = raw.to_ascii_lowercase();
    assert!(raw.starts_with("POST /api/agent-control/runs/import "));
    assert!(lower.contains("content-type: multipart/form-data"));
    assert!(lower.contains("idempotency-key: warpspeed-key-1"));
    assert!(lower.contains("x-stozn-client: warpspeed"));
    assert!(lower.contains("authorization: bearer token"));
    for part in [
        "agent_name",
        "session_id",
        "repo_url",
        "handoff_branch",
        "source_cwd",
        "session_file",
    ] {
        assert!(
            raw.contains(&format!("name=\"{part}\"")),
            "missing part {part}"
        );
    }
    assert!(raw.contains("filename=\"session.jsonl\""));
    assert!(raw.contains("handoff/abc-123"));
}

#[test]
fn import_refuses_oversized_file_before_sending() {
    let profile = fixture_profile("http://127.0.0.1:1");
    let error = import_moved_session_with(
        &profile,
        "token",
        moved_import(vec![0; MAX_SESSION_FILE_BYTES + 1]),
    )
    .unwrap_err();
    assert!(error.to_string().contains("32 MiB"), "{error}");
}

fn resumed_page(content: &str, terminal: bool) -> serde_json::Value {
    json!({
        "items": [{
            "event_id": "e1", "job_id": "job-1", "sequence": 1, "timestamp": "t",
            "type": "status", "content": content, "redacted": false, "truncated": false
        }],
        "next_cursor": "c1",
        "terminal": terminal,
        "gap": false,
        "earliest_cursor": null
    })
}

#[test]
fn resume_wait_succeeds_on_resumed_status() {
    let profile = fixture_profile(http_fixture("200 OK", resumed_page("resumed", false)));
    let client = Client::new();
    wait_until_resumed_with(
        &client,
        &profile,
        "token",
        "job-1",
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
}

#[test]
fn resume_wait_fails_on_terminal_page() {
    let profile = fixture_profile(http_fixture("200 OK", resumed_page("started", true)));
    let client = Client::new();
    let error = wait_until_resumed_with(
        &client,
        &profile,
        "token",
        "job-1",
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("ended before resuming"),
        "{error}"
    );
}

type RequestLog = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

/// Serves `responses` in order, one per connection, repeating the last one; logs request lines.
fn sequence_fixture(
    responses: Vec<(&'static str, serde_json::Value)>,
) -> (&'static str, RequestLog) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let log = RequestLog::default();
    let served = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 8192];
            let read = stream.read(&mut request).unwrap();
            let line = String::from_utf8_lossy(&request[..read])
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let index = {
                let mut served = served.lock().unwrap();
                served.push(line);
                served.len() - 1
            };
            let (status, body) = &responses[index.min(responses.len() - 1)];
            let body = body.to_string();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            stream.flush().unwrap();
        }
    });
    (Box::leak(format!("http://{address}").into_boxed_str()), log)
}

fn empty_page() -> serde_json::Value {
    json!({
        "items": [],
        "next_cursor": null,
        "terminal": false,
        "gap": false,
        "earliest_cursor": null
    })
}

#[test]
fn resume_wait_retries_a_transient_error() {
    let (base, log) = sequence_fixture(vec![
        ("503 Service Unavailable", json!({ "error": "busy" })),
        ("200 OK", resumed_page("resumed", false)),
    ]);
    let profile = fixture_profile(base);
    wait_until_resumed_with(
        &Client::new(),
        &profile,
        "token",
        "job-1",
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[test]
fn resume_wait_gives_up_at_the_deadline_without_spinning() {
    let (base, log) = sequence_fixture(vec![("200 OK", empty_page())]);
    let profile = fixture_profile(base);
    let error = wait_until_resumed_with(
        &Client::new(),
        &profile,
        "token",
        "job-1",
        Instant::now() + Duration::from_millis(1200),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("did not resume within"),
        "{error}"
    );
    let requests = log.lock().unwrap().len();
    assert!(requests <= 5, "polled {requests} times in 1.2s");
}

#[test]
fn resume_wait_fails_fast_on_a_client_error() {
    for status in [
        "401 Unauthorized",
        "403 Forbidden",
        "404 Not Found",
        "422 Unprocessable Entity",
    ] {
        let (base, log) = sequence_fixture(vec![(status, json!({ "error": "denied" }))]);
        let profile = fixture_profile(base);
        let started = Instant::now();
        let error = wait_until_resumed_with(
            &Client::new(),
            &profile,
            "token",
            "job-1",
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap_err();
        let code = &status[..3];
        assert!(format!("{error:#}").contains(code), "{error:#}");
        assert!(started.elapsed() < Duration::from_secs(2), "{status}");
        assert_eq!(log.lock().unwrap().len(), 1, "{status}");
    }
}

#[test]
fn resume_wait_retries_rate_limiting() {
    let (base, log) = sequence_fixture(vec![
        ("429 Too Many Requests", json!({ "error": "slow down" })),
        ("200 OK", resumed_page("resumed", false)),
    ]);
    let profile = fixture_profile(base);
    wait_until_resumed_with(
        &Client::new(),
        &profile,
        "token",
        "job-1",
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[test]
fn resume_wait_restarts_without_a_cursor_after_a_history_gap() {
    let gap_page = json!({
        "items": [],
        "next_cursor": "c9",
        "terminal": false,
        "gap": true,
        "earliest_cursor": "c5"
    });
    let (base, log) = sequence_fixture(vec![
        (
            "200 OK",
            json!({
                "items": [], "next_cursor": "c1", "terminal": false, "gap": false,
                "earliest_cursor": null
            }),
        ),
        ("410 Gone", gap_page),
        ("200 OK", resumed_page("resumed", false)),
    ]);
    let profile = fixture_profile(base);
    wait_until_resumed_with(
        &Client::new(),
        &profile,
        "token",
        "job-1",
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 3);
    assert!(log[1].contains("after=c1"), "{}", log[1]);
    assert!(!log[2].contains("after="), "{}", log[2]);
}

#[test]
fn stop_run_posts_a_stop_request() {
    let (base, request) = capturing_fixture(json!({ "status": "accepted" }));
    let profile = fixture_profile(base);
    stop_run_with(&profile, "token", "job-1", true).unwrap();
    let raw = String::from_utf8_lossy(&request.recv().unwrap()).into_owned();
    assert!(
        raw.starts_with("POST /api/agent-control/runs/job-1/stop "),
        "{raw}"
    );
    assert!(raw.contains("\"force\":true"), "{raw}");
}
