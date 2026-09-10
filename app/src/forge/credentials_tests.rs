use super::{Credential, CredentialChain, CredentialStore, os_store_targets};

/// A store that answers for exactly one host.
struct FakeStore {
    name: &'static str,
    host: &'static str,
    token: &'static str,
}

impl CredentialStore for FakeStore {
    fn credential(&self, host: &str) -> Option<Credential> {
        (host == self.host).then(|| Credential {
            username: None,
            token: self.token.to_owned(),
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

fn store(name: &'static str, token: &'static str) -> Box<dyn CredentialStore> {
    Box::new(FakeStore {
        name,
        host: "example.com",
        token,
    })
}

#[test]
fn chain_returns_the_first_store_that_answers() {
    let chain = CredentialChain::new(vec![store("first", "one"), store("second", "two")]);
    assert_eq!(chain.resolve("example.com").unwrap().token, "one");
}

/// The point of the order is that an earlier store can correct a later one, so
/// a miss must fall through rather than end the search.
#[test]
fn chain_falls_through_a_store_that_does_not_answer() {
    struct NeverAnswers;
    impl CredentialStore for NeverAnswers {
        fn credential(&self, _host: &str) -> Option<Credential> {
            None
        }
        fn name(&self) -> &'static str {
            "never"
        }
    }

    let chain = CredentialChain::new(vec![Box::new(NeverAnswers), store("second", "two")]);
    assert_eq!(chain.resolve("example.com").unwrap().token, "two");
}

/// No credential anywhere is "unauthenticated", which the caller distinguishes
/// from a failure. It must not be reported as an error.
#[test]
fn chain_resolves_to_nothing_for_an_unknown_host() {
    let chain = CredentialChain::new(vec![store("first", "one")]);
    assert!(chain.resolve("other.example.com").is_none());
}

#[test]
fn chain_with_no_stores_resolves_to_nothing() {
    let chain = CredentialChain::new(Vec::new());
    assert!(chain.resolve("example.com").is_none());
}

#[test]
fn os_store_targets_try_the_gh_token_before_the_git_credential() {
    assert_eq!(
        os_store_targets("github.com", Some("budelius")),
        vec![
            "gh:github.com:budelius".to_string(),
            "gh:github.com:".to_string(),
            "git:https://github.com".to_string(),
        ]
    );
}

/// The username comes from `gh`'s config file, which may be absent or blank.
/// The user-less entry and the git credential still have to be tried.
#[test]
fn os_store_targets_without_a_username_still_try_the_remaining_entries() {
    assert_eq!(
        os_store_targets("gitlab.com", None),
        vec![
            "gh:gitlab.com:".to_string(),
            "git:https://gitlab.com".to_string(),
        ]
    );
    assert_eq!(
        os_store_targets("gitlab.com", Some("   ")),
        os_store_targets("gitlab.com", None)
    );
}

/// A token must never be printable, because these values reach logs and panic
/// messages by accident.
#[test]
fn credential_debug_redacts_the_token() {
    let credential = Credential {
        username: Some("budelius".to_string()),
        token: "ghp_supersecret".to_string(),
    };
    let rendered = format!("{credential:?}");
    assert!(
        !rendered.contains("ghp_supersecret"),
        "token leaked: {rendered}"
    );
    assert!(
        rendered.contains("budelius"),
        "username should still be visible: {rendered}"
    );
}
