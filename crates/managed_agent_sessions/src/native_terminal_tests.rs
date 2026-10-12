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
