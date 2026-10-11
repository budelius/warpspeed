use std::collections::HashMap;
use std::ffi::OsString;

use warp_core::SessionId;
use warp_core::cli_agent_protocol::{WARP_AGENT_EVENT_BIN_ENV, WARP_AGENT_EVENT_TOKEN_ENV};

use super::{SECRET_LEN, insert_env_vars, load_or_create_secret, token_from_secret};

#[test]
fn token_is_128_bit_hex_and_stable_per_session() {
    let secret = [7u8; SECRET_LEN];
    let token = token_from_secret(&secret, SessionId::from(42));
    assert_eq!(token.len(), 32);
    assert!(
        token
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(token, token_from_secret(&secret, SessionId::from(42)));
}

#[test]
fn token_differs_by_session_and_by_secret() {
    let secret = [7u8; SECRET_LEN];
    let other_secret = [8u8; SECRET_LEN];
    let token = token_from_secret(&secret, SessionId::from(42));
    assert_ne!(token, token_from_secret(&secret, SessionId::from(43)));
    assert_ne!(token, token_from_secret(&other_secret, SessionId::from(42)));
}

#[test]
fn secret_is_created_once_and_then_reused() {
    let dir = tempfile::tempdir().unwrap();
    let first = load_or_create_secret(dir.path()).unwrap();
    let second = load_or_create_secret(dir.path()).unwrap();
    assert_eq!(first.len(), SECRET_LEN);
    assert_eq!(first, second);
}

#[test]
fn a_truncated_secret_file_is_not_used() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(super::SECRET_FILE_NAME), b"short").unwrap();
    assert!(load_or_create_secret(dir.path()).is_err());
}

#[test]
fn env_vars_carry_the_token_and_binary() {
    let mut env_vars = HashMap::new();
    insert_env_vars(
        &mut env_vars,
        "0123456789abcdef0123456789abcdef".to_owned(),
        std::path::Path::new(r"C:\Program Files\WarpSpeed\warpspeed.exe"),
    );
    assert_eq!(
        env_vars.get(&OsString::from(WARP_AGENT_EVENT_TOKEN_ENV)),
        Some(&OsString::from("0123456789abcdef0123456789abcdef"))
    );
    assert_eq!(
        env_vars.get(&OsString::from(WARP_AGENT_EVENT_BIN_ENV)),
        Some(&OsString::from(r"C:\Program Files\WarpSpeed\warpspeed.exe"))
    );
}
