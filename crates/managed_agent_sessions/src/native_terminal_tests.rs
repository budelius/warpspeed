use base64::Engine as _;

use super::*;

fn public_key() -> String {
    let mut blob = Vec::new();
    blob.extend_from_slice(&11_u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32_u32.to_be_bytes());
    blob.extend_from_slice(&[3_u8; 32]);
    format!(
        "ssh-ed25519 {}",
        base64::engine::general_purpose::STANDARD.encode(blob)
    )
}

fn endpoint() -> SshEndpoint {
    SshEndpoint {
        host: "10.244.7.25".to_string(),
        port: SSH_PORT,
        user: SSH_USER.to_string(),
        host_public_key: public_key(),
    }
}

fn test_files() -> EphemeralSshFiles {
    let directory =
        std::env::temp_dir().join(format!("warpspeed-terminal-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&directory).unwrap();
    let private_key = directory.join("identity");
    let known_hosts = directory.join("known_hosts");
    fs::write(&private_key, "not-a-real-private-key").unwrap();
    EphemeralSshFiles {
        directory,
        private_key,
        known_hosts,
        public_key: public_key(),
    }
}

#[test]
fn gateway_ticket_accepts_only_the_pinned_short_lived_endpoint() {
    let now = DateTime::parse_from_rfc3339("2026-10-28T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let ticket = TerminalGatewayTicket {
        protocol: TERMINAL_GATEWAY_PROTOCOL.to_string(),
        ticket: "payload.signature".to_string(),
        expires_at: "2026-10-28T12:00:30Z".to_string(),
        websocket_url: TERMINAL_GATEWAY_SOCKET_URL.to_string(),
    };

    assert!(validate_gateway_ticket(&ticket, now).is_ok());

    let wrong_endpoint = TerminalGatewayTicket {
        websocket_url: "wss://example.test/api/agent-control/terminal/socket".to_string(),
        ..ticket
    };
    assert!(validate_gateway_ticket(&wrong_endpoint, now).is_err());
}

#[test]
fn gateway_ticket_rejects_expired_or_header_unsafe_credentials() {
    let now = DateTime::parse_from_rfc3339("2026-10-28T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let expired = TerminalGatewayTicket {
        protocol: TERMINAL_GATEWAY_PROTOCOL.to_string(),
        ticket: "payload.signature".to_string(),
        expires_at: "2026-10-28T12:00:00Z".to_string(),
        websocket_url: TERMINAL_GATEWAY_SOCKET_URL.to_string(),
    };
    assert!(validate_gateway_ticket(&expired, now).is_err());

    let unsafe_ticket = TerminalGatewayTicket {
        ticket: "payload,another-protocol".to_string(),
        expires_at: "2026-10-28T12:00:30Z".to_string(),
        ..expired
    };
    assert!(validate_gateway_ticket(&unsafe_ticket, now).is_err());
}

#[test]
fn gateway_retries_only_transient_ticket_failures() {
    for status in [
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let error = anyhow::Error::new(super::super::HttpStatusError {
            status,
            body: String::new(),
        });
        assert!(gateway_ticket_error_is_retryable(&error));
    }

    let rejected = anyhow::Error::new(super::super::HttpStatusError {
        status: StatusCode::UNAUTHORIZED,
        body: String::new(),
    });
    assert!(!gateway_ticket_error_is_retryable(&rejected));
}

#[test]
fn gateway_output_suppresses_terminal_identity_queries() {
    let mut filter = TerminalOutputFilter::default();

    assert_eq!(filter.push(b"prefix\x1b[>"), b"prefix");
    assert_eq!(filter.push(b"cmiddle\x1b[>0"), b"middle");
    assert_eq!(
        filter.push(b"qsuffix\x1b[?25h\x1b[>4;2m"),
        b"suffix\x1b[?25h\x1b[>4;2m"
    );
}

#[test]
fn gateway_keepalive_is_a_ping_control_message() {
    assert_eq!(gateway_keepalive().text(), Some(r#"{"type":"ping"}"#));
}

#[test]
fn gateway_idle_and_duration_limits_trigger_reconnection() {
    for code in ["idle_timeout", "duration_limit"] {
        let message = Message::new_text(
            json!({
                "type": "error",
                "code": code,
                "message": "Live terminal disconnected",
            })
            .to_string(),
        );

        assert_eq!(
            gateway_message(
                message,
                "job-123",
                "generation-123",
                true,
                &mut TerminalOutputFilter::default(),
            )
            .unwrap(),
            GatewayMessage::Reconnect
        );
    }
}

#[test]
fn gateway_remote_exit_triggers_reconnection() {
    let message = Message::new_text(json!({ "type": "exit", "code": 0 }).to_string());

    assert_eq!(
        gateway_message(
            message,
            "job-123",
            "generation-123",
            true,
            &mut TerminalOutputFilter::default(),
        )
        .unwrap(),
        GatewayMessage::Reconnect
    );
}

#[test]
fn gateway_interruptions_reconnect_during_initial_handshake() {
    let message = gateway_message(
        Message::new_text(json!({ "type": "exit", "code": 0 }).to_string()),
        "job-123",
        "generation-123",
        false,
        &mut TerminalOutputFilter::default(),
    )
    .unwrap();

    assert_eq!(
        gateway_terminal_outcome(&message),
        Some(GatewayTerminalOutcome::Reconnect)
    );
}

#[test]
fn gateway_writer_conflicts_remain_distinct_from_reconnects() {
    let message = Message::new_text(
        json!({
            "type": "error",
            "code": "writer_exists",
            "message": "Live terminal is open elsewhere",
        })
        .to_string(),
    );

    assert_eq!(
        gateway_message(
            message,
            "job-123",
            "generation-123",
            false,
            &mut TerminalOutputFilter::default(),
        )
        .unwrap(),
        GatewayMessage::WriterExists
    );
}

#[test]
fn local_terminal_input_detaches_only_on_control_close_bracket() {
    let detach = Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL));
    assert_eq!(local_terminal_input(detach), LocalTerminalInput::Detach);

    let close_bracket = Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    assert_eq!(
        local_terminal_input(close_bracket),
        LocalTerminalInput::Bytes(b"]".to_vec())
    );
}

#[test]
fn terminal_keys_are_encoded_for_the_remote_pty() {
    assert_eq!(
        encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL)),
        Some(b"\x1b[1;5A".to_vec())
    );
    assert_eq!(
        encode_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(vec![0x03])
    );
    assert_eq!(
        encode_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)),
        Some(b"\x1bx".to_vec())
    );
}

#[test]
fn endpoint_validation_accepts_only_the_fixed_private_target() {
    assert!(validate_endpoint(&endpoint()).is_ok());
    for invalid in [
        SshEndpoint {
            host: "203.0.113.20".to_string(),
            ..endpoint()
        },
        SshEndpoint {
            port: 22,
            ..endpoint()
        },
        SshEndpoint {
            user: "root".to_string(),
            ..endpoint()
        },
        SshEndpoint {
            host_public_key: format!("{} comment", public_key()),
            ..endpoint()
        },
    ] {
        assert!(validate_endpoint(&invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn ssh_arguments_pin_identity_host_key_and_disable_broad_features() {
    let files = test_files();
    let endpoint = endpoint();
    let command = ssh_command(&files, &endpoint);
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(&args[..2], ["-F", "none"]);
    for expected in [
        "-tt",
        "BatchMode=yes",
        "IdentitiesOnly=yes",
        "IdentityAgent=none",
        "PreferredAuthentications=publickey",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "GSSAPIAuthentication=no",
        "HostbasedAuthentication=no",
        "StrictHostKeyChecking=yes",
        "UpdateHostKeys=no",
        "HostKeyAlgorithms=ssh-ed25519",
        "PubkeyAcceptedAlgorithms=ssh-ed25519",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "ForwardAgent=no",
        "ClearAllForwardings=yes",
        "PermitLocalCommand=no",
        "ControlMaster=no",
        "ControlPath=none",
        "ControlPersist=no",
        "RequestTTY=force",
        "agentctrl@10.244.7.25",
    ] {
        assert!(
            args.iter().any(|argument| argument == expected),
            "{expected}: {args:?}"
        );
    }
    assert!(
        args.iter()
            .any(|argument| argument.starts_with("UserKnownHostsFile="))
    );
    assert_eq!(args.last().unwrap(), "agentctrl@10.244.7.25");
    assert!(!args.iter().any(|argument| argument.contains("tmux")));
    assert!(!args.iter().any(|argument| argument.contains("job-")));
}

#[test]
fn known_hosts_file_contains_only_the_exact_bracketed_endpoint() {
    let files = test_files();
    files.write_known_host(&endpoint()).unwrap();
    let line = fs::read_to_string(&files.known_hosts).unwrap();
    assert_eq!(line, format!("[10.244.7.25]:2222 {}\n", public_key()));
}

#[test]
fn authorization_response_must_echo_every_binding() {
    let valid = AuthorizationResponse {
        authorization_id: "authorization-123".to_string(),
        job_id: "job-123".to_string(),
        execution_generation: "pi-123".to_string(),
        client_id: "client-123".to_string(),
        expires_at_ms: 1,
        lease_renew_after_ms: 20_000,
        takeover: false,
        endpoint: endpoint(),
    };
    assert!(validate_authorization(&valid, "job-123", "pi-123", "client-123", false).is_ok());
    let mut wrong = valid;
    wrong.execution_generation = "pi-old".to_string();
    assert!(validate_authorization(&wrong, "job-123", "pi-123", "client-123", false).is_err());
}

#[test]
fn descriptor_requests_cannot_extend_the_thirty_second_poll_deadline() {
    assert_eq!(
        descriptor_request_timeout(Duration::from_secs(8)),
        Some(Duration::from_secs(5))
    );
    assert_eq!(
        descriptor_request_timeout(Duration::from_millis(250)),
        Some(Duration::from_millis(250))
    );
    assert_eq!(descriptor_request_timeout(Duration::ZERO), None);
}

#[test]
fn renewal_interval_is_never_longer_than_twenty_seconds() {
    let response = AuthorizationResponse {
        authorization_id: "authorization-123".to_string(),
        job_id: "job-123".to_string(),
        execution_generation: "pi-123".to_string(),
        client_id: "client-123".to_string(),
        expires_at_ms: 1,
        lease_renew_after_ms: u64::MAX,
        takeover: false,
        endpoint: endpoint(),
    };
    assert_eq!(renewal_interval(&response), Duration::from_secs(20));
}

#[test]
fn terminal_authorization_errors_distinguish_retry_fallback_and_writer_conflict() {
    let error = anyhow::Error::new(super::super::HttpStatusError {
        status: StatusCode::CONFLICT,
        body: r#"{"code":"stale_generation"}"#.to_string(),
    });
    assert!(is_stale_generation(&error));
    assert!(unavailable_reason(&error).is_none());

    let error = anyhow::Error::new(super::super::HttpStatusError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        body: r#"{"code":"control_unavailable"}"#.to_string(),
    });
    assert!(!is_stale_generation(&error));
    assert!(unavailable_reason(&error).is_some());

    let error = anyhow::Error::new(super::super::HttpStatusError {
        status: StatusCode::CONFLICT,
        body: r#"{"code":"terminal_writer_exists"}"#.to_string(),
    });
    assert!(!is_stale_generation(&error));
    assert!(unavailable_reason(&error).is_none());
}

#[test]
fn platform_null_known_hosts_path_is_explicit() {
    #[cfg(windows)]
    assert_eq!(null_known_hosts_option(), "GlobalKnownHostsFile=NUL");
    #[cfg(not(windows))]
    assert_eq!(null_known_hosts_option(), "GlobalKnownHostsFile=/dev/null");
}
