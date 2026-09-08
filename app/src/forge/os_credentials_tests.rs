use super::{decode_credential_blob, username_from_gh_hosts};

/// The shape `gh` actually writes on a machine where the token lives in the
/// keyring: a `users:` block listing accounts, and a separate `user:` naming
/// the active one.
const HOSTS_YML: &str = "\
github.com:
    git_protocol: https
    users:
        budelius:
    user: budelius
gitlab.example.com:
    user: someone-else
";

#[test]
fn username_from_gh_hosts_reads_the_active_user_for_a_host() {
    assert_eq!(
        username_from_gh_hosts(HOSTS_YML, "github.com").as_deref(),
        Some("budelius")
    );
}

/// Two hosts in one file must not bleed into each other.
#[test]
fn username_from_gh_hosts_scopes_the_lookup_to_the_host_block() {
    assert_eq!(
        username_from_gh_hosts(HOSTS_YML, "gitlab.example.com").as_deref(),
        Some("someone-else")
    );
    assert_eq!(
        username_from_gh_hosts(HOSTS_YML, "absent.example.com"),
        None
    );
}

/// `users:` opens the account list and is not the active user. Matching it
/// would yield an empty name and lose the real one.
#[test]
fn username_from_gh_hosts_does_not_mistake_the_users_block_for_the_user_key() {
    let yaml = "github.com:\n    users:\n        budelius:\n";
    assert_eq!(username_from_gh_hosts(yaml, "github.com"), None);
}

#[test]
fn decode_credential_blob_reads_utf8() {
    assert_eq!(
        decode_credential_blob(b"gho_token").as_deref(),
        Some("gho_token")
    );
}

/// Git Credential Manager writes UTF-16LE, so every second byte is zero.
#[test]
fn decode_credential_blob_reads_utf16le() {
    let utf16: Vec<u8> = "gho_token"
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    assert_eq!(decode_credential_blob(&utf16).as_deref(), Some("gho_token"));
}

#[test]
fn decode_credential_blob_rejects_an_empty_or_blank_blob() {
    assert_eq!(decode_credential_blob(b""), None);
    assert_eq!(decode_credential_blob(b"   "), None);
    assert_eq!(decode_credential_blob(b"\0\0"), None);
}

/// Reads the machine's real credential store, so it is ignored by default and
/// never runs in CI. Run it by hand to confirm the `CredReadW` binding actually
/// returns something:
///
/// ```text
/// cargo test -p warp --lib forge::os_credentials -- --ignored --nocapture
/// ```
///
/// It asserts only on presence and shape. A token must never be printed.
#[test]
#[ignore = "reads the machine's real credential store"]
fn os_credential_store_finds_a_token_on_this_machine() {
    use super::super::credentials::CredentialStore as _;
    use super::OsCredentialStore;

    let found = OsCredentialStore.credential("github.com");
    match found {
        Some(credential) => {
            assert!(
                credential.token.len() > 8,
                "a token that short is not plausible"
            );
            println!(
                "found a github.com credential: user={:?}, token length {}",
                credential.username,
                credential.token.len()
            );
        }
        None => panic!("no github.com credential found in the OS store"),
    }
}
