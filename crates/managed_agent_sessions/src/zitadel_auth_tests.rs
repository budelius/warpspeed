use std::io::{Read as _, Write as _};
use std::net::TcpListener;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::json;

use super::*;

/// Serves `responses` in order, one per connection, repeating the last; records request bodies.
fn token_server(
    responses: Vec<(&'static str, serde_json::Value)>,
) -> (&'static str, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    token_server_with_hook(responses, || {})
}

/// Like `token_server`, running `before_response` after each request is read.
fn token_server_with_hook(
    responses: Vec<(&'static str, serde_json::Value)>,
    before_response: impl Fn() + Send + 'static,
) -> (&'static str, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let served = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 16_384];
            let read = stream.read(&mut request).unwrap();
            let text = String::from_utf8_lossy(&request[..read]).into_owned();
            let index = {
                let mut served = served.lock().unwrap();
                served.push(text);
                served.len() - 1
            };
            before_response();
            let (status, body) = &responses[index.min(responses.len() - 1)];
            let body = body
                .to_string()
                .replace("{issuer}", &format!("http://{address}"));
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (Box::leak(format!("http://{address}").into_boxed_str()), log)
}

fn access_token(roles: &[&str], exp_in: i64) -> String {
    let role_map: serde_json::Map<String, serde_json::Value> = roles
        .iter()
        .map(|role| ((*role).to_owned(), json!({ "org": "obachan" })))
        .collect();
    let payload = json!({
        "sub": "2847",
        "exp": chrono::Utc::now().timestamp() + exp_in,
        "urn:zitadel:iam:org:project:proj-1:roles": role_map,
    });
    format!(
        "{}.{}.sig",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}

fn profile(issuer: &'static str) -> ZitadelProfile {
    ZitadelProfile {
        issuer,
        client_id: "client-1",
        project_id: "proj-1",
    }
}

#[test]
fn device_start_requests_project_audience_and_validates_the_verification_url() {
    let (issuer, log) = token_server(vec![(
        "200 OK",
        json!({
            "device_code": "dev-1",
            "user_code": "ABCD-EFGH",
            "verification_uri": "{issuer}/device",
            "verification_uri_complete": "{issuer}/device?user_code=ABCD-EFGH",
            "expires_in": 300,
            "interval": 5
        }),
    )]);
    let start = start_device_login(&profile(issuer)).unwrap();
    assert_eq!(start.user_code, "ABCD-EFGH");
    assert_eq!(
        start.verification_url,
        format!("{issuer}/device?user_code=ABCD-EFGH")
    );
    let request = &log.lock().unwrap()[0];
    assert!(request.starts_with("POST /oauth/v2/device_authorization"));
    assert!(request.contains("client_id=client-1"));
    assert!(request.contains("urn%3Azitadel%3Aiam%3Aorg%3Aproject%3Aid%3Aproj-1%3Aaud"));
    assert!(request.contains("offline_access"));
}

#[test]
fn device_start_refuses_a_verification_url_on_another_origin() {
    let (issuer, _) = token_server(vec![(
        "200 OK",
        json!({
            "device_code": "dev-1",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://evil.example/device",
            "expires_in": 300,
            "interval": 5
        }),
    )]);
    assert!(start_device_login(&profile(issuer)).is_err());
}

#[test]
fn polling_waits_through_pending_and_slow_down_then_returns_tokens() {
    let token = access_token(&["agent-admin"], 3600);
    let (issuer, log) = token_server(vec![
        (
            "400 Bad Request",
            json!({ "error": "authorization_pending" }),
        ),
        ("400 Bad Request", json!({ "error": "slow_down" })),
        (
            "200 OK",
            json!({ "access_token": token, "refresh_token": "ref-1", "expires_in": 3600, "token_type": "Bearer" }),
        ),
    ]);
    let tokens = poll_device_login(
        &profile(issuer),
        "dev-1",
        Duration::from_millis(1),
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(tokens.refresh_token, "ref-1");
    assert_eq!(log.lock().unwrap().len(), 3);
}

#[test]
fn polling_stops_on_access_denied() {
    let (issuer, _) = token_server(vec![(
        "400 Bad Request",
        json!({ "error": "access_denied" }),
    )]);
    let error = poll_device_login(
        &profile(issuer),
        "dev-1",
        Duration::from_millis(1),
        Duration::from_secs(10),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("denied"));
}

#[test]
fn roles_map_to_admin_user_or_no_access() {
    let admin = identity_from_access_token(
        &access_token(&["agent-user", "agent-admin"], 60),
        "proj-1",
        "id-dev.obachan.dev",
    )
    .unwrap();
    assert_eq!(admin.role, "admin");
    assert_eq!(admin.subject, "2847");
    let user = identity_from_access_token(
        &access_token(&["agent-user"], 60),
        "proj-1",
        "id-dev.obachan.dev",
    )
    .unwrap();
    assert_eq!(user.role, "user");
    let error = identity_from_access_token(&access_token(&[], 60), "proj-1", "id-dev.obachan.dev")
        .unwrap_err();
    assert_eq!(
        format!("{error}"),
        "Your account has no access to the agent platform (id-dev.obachan.dev)"
    );
}

#[test]
fn refresh_rotates_the_refresh_token() {
    let token = access_token(&["agent-user"], 3600);
    let (issuer, log) = token_server(vec![(
        "200 OK",
        json!({ "access_token": token, "refresh_token": "ref-2", "expires_in": 3600 }),
    )]);
    let tokens = refresh(&profile(issuer), "ref-1").unwrap();
    assert_eq!(tokens.refresh_token, "ref-2");
    let request = &log.lock().unwrap()[0];
    assert!(request.contains("grant_type=refresh_token"));
    assert!(request.contains("refresh_token=ref-1"));
}

#[test]
fn refresh_without_a_new_refresh_token_keeps_the_old_one() {
    let token = access_token(&["agent-user"], 3600);
    let (issuer, _) = token_server(vec![(
        "200 OK",
        json!({ "access_token": token, "expires_in": 3600 }),
    )]);
    assert_eq!(
        refresh(&profile(issuer), "ref-1").unwrap().refresh_token,
        "ref-1"
    );
}

#[test]
fn needs_refresh_inside_the_sixty_second_window() {
    let now = chrono::Utc::now();
    assert!(needs_refresh(now + chrono::Duration::seconds(59), now));
    assert!(!needs_refresh(now + chrono::Duration::seconds(61), now));
}

#[test]
fn refresh_server_errors_are_not_a_rejected_sign_in() {
    for status in ["503 Service Unavailable", "429 Too Many Requests"] {
        let (issuer, _) = token_server(vec![(
            status,
            json!({ "error": "temporarily_unavailable" }),
        )]);
        let error = refresh(&profile(issuer), "ref-1").unwrap_err();
        assert!(
            error.downcast_ref::<RefreshRejected>().is_none(),
            "{status}"
        );
    }
    let (issuer, _) = token_server(vec![(
        "400 Bad Request",
        json!({ "error": "invalid_request" }),
    )]);
    let error = refresh(&profile(issuer), "ref-1").unwrap_err();
    assert!(error.downcast_ref::<RefreshRejected>().is_none());
}

#[test]
fn refresh_invalid_grant_is_a_rejected_sign_in() {
    for status in ["400 Bad Request", "401 Unauthorized"] {
        let (issuer, _) = token_server(vec![(status, json!({ "error": "invalid_grant" }))]);
        let error = refresh(&profile(issuer), "ref-1").unwrap_err();
        assert!(
            error.downcast_ref::<RefreshRejected>().is_some(),
            "{status}"
        );
    }
}

mod stored_sign_in {
    use std::path::Path;

    use super::*;
    use crate::{
        STOZN_AUTH_FORMAT, StoredStoznAuth, access_token_at, load_stozn_auth_at, save_stozn_auth_at,
    };

    fn temp_auth_path() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("stozn-auth-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("stozn-auth.bin")
    }

    fn save(path: &Path, access_token: String, expires_in: i64, refresh_token: &str) {
        save_stozn_auth_at(
            path,
            &StoredStoznAuth {
                format_version: STOZN_AUTH_FORMAT,
                environment_alias: "staging".to_owned(),
                access_token,
                access_expires_at: (Utc::now() + chrono::Duration::seconds(expires_in))
                    .to_rfc3339(),
                refresh_token: refresh_token.to_owned(),
                user: StoznIdentity {
                    subject: "2847".to_owned(),
                    role: "user".to_owned(),
                    display_name: None,
                },
            },
        )
        .unwrap();
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn fresh_token_is_returned_without_contacting_the_issuer() {
        let path = temp_auth_path();
        save(&path, "stored-access".to_owned(), 3600, "ref-1");
        let token = access_token_at(&path, &profile("http://127.0.0.1:1")).unwrap();
        assert_eq!(token, "stored-access");
        cleanup(&path);
    }

    #[test]
    fn expiring_token_is_refreshed_and_the_rotated_refresh_token_is_saved() {
        let path = temp_auth_path();
        save(&path, "old-access".to_owned(), 10, "ref-1");
        let new_access = access_token(&["agent-admin"], 3600);
        let (issuer, _) = token_server(vec![(
            "200 OK",
            json!({ "access_token": new_access, "refresh_token": "ref-2", "expires_in": 3600 }),
        )]);
        let token = access_token_at(&path, &profile(issuer)).unwrap();
        assert_eq!(token, new_access);
        let stored = load_stozn_auth_at(&path).unwrap();
        assert_eq!(stored.refresh_token, "ref-2");
        assert_eq!(stored.access_token, new_access);
        assert_eq!(stored.user.role, "admin");
        cleanup(&path);
    }

    #[test]
    fn rotated_refresh_token_is_saved_even_when_the_role_is_missing() {
        let path = temp_auth_path();
        save(&path, "old-access".to_owned(), 10, "ref-1");
        let new_access = access_token(&[], 3600);
        let (issuer, _) = token_server(vec![(
            "200 OK",
            json!({ "access_token": new_access, "refresh_token": "ref-2", "expires_in": 3600 }),
        )]);
        assert!(access_token_at(&path, &profile(issuer)).is_err());
        let stored = load_stozn_auth_at(&path).unwrap();
        assert_eq!(stored.refresh_token, "ref-2");
        assert_eq!(stored.user.role, "user");
        cleanup(&path);
    }

    #[test]
    fn rejected_refresh_with_an_unchanged_token_signs_out() {
        let path = temp_auth_path();
        save(&path, "old-access".to_owned(), 10, "ref-1");
        let (issuer, _) = token_server(vec![(
            "400 Bad Request",
            json!({ "error": "invalid_grant" }),
        )]);
        let error = access_token_at(&path, &profile(issuer)).unwrap_err();
        assert!(error.downcast_ref::<RefreshRejected>().is_some());
        assert!(!path.exists());
        cleanup(&path);
    }

    #[test]
    fn rejected_refresh_after_another_process_rotated_uses_the_newer_sign_in() {
        let path = temp_auth_path();
        save(&path, "old-access".to_owned(), 10, "ref-1");
        let rotated_path = path.clone();
        let (issuer, _) = token_server_with_hook(
            vec![("400 Bad Request", json!({ "error": "invalid_grant" }))],
            move || save(&rotated_path, "rotated-access".to_owned(), 3600, "ref-2"),
        );
        let token = access_token_at(&path, &profile(issuer)).unwrap();
        assert_eq!(token, "rotated-access");
        assert!(path.exists());
        cleanup(&path);
    }

    #[test]
    fn server_error_during_refresh_keeps_the_stored_sign_in() {
        let path = temp_auth_path();
        save(&path, "old-access".to_owned(), 10, "ref-1");
        let (issuer, _) = token_server(vec![("503 Service Unavailable", json!({}))]);
        let error = access_token_at(&path, &profile(issuer)).unwrap_err();
        assert!(error.downcast_ref::<RefreshRejected>().is_none());
        assert_eq!(load_stozn_auth_at(&path).unwrap().refresh_token, "ref-1");
        cleanup(&path);
    }
}
