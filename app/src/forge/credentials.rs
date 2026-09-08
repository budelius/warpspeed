//! Finding a token for a forge host, from credentials already on the machine.
//!
//! Three sources, consulted in order, first hit wins: an explicit per-host
//! override in Warp settings, a PAT the user handed Warp directly, and finally
//! whatever the OS credential store already holds. The order is what makes
//! "all of them" unambiguous — an override can always correct a wrong guess.
//!
//! A miss at every layer is not an error. It means *unauthenticated*, which the
//! UI answers with a sign-in affordance rather than a failure toast.
//!
//! This module only ever reads. It never creates, refreshes, or stores a token.

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;

/// A token for one host, and the username it belongs to when the store knows it.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub username: Option<String>,
    pub token: String,
}

// A token must never reach a log or a panic message.
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("username", &self.username)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// One place a credential might be found.
pub trait CredentialStore: Send + Sync {
    /// The credential this store holds for `host`, if any.
    fn credential(&self, host: &str) -> Option<Credential>;

    /// Names the store in logs. Never used to make decisions.
    fn name(&self) -> &'static str;
}

/// The ordered chain of stores.
pub struct CredentialChain {
    stores: Vec<Box<dyn CredentialStore>>,
}

impl CredentialChain {
    pub fn new(stores: Vec<Box<dyn CredentialStore>>) -> Self {
        Self { stores }
    }

    /// The first credential any store holds for `host`.
    ///
    /// `None` means unauthenticated, not failed.
    pub fn resolve(&self, host: &str) -> Option<Credential> {
        for store in &self.stores {
            if let Some(credential) = store.credential(host) {
                log::debug!("forge: using the {} credential for {host}", store.name());
                return Some(credential);
            }
        }
        log::debug!("forge: no credential for {host}");
        None
    }
}

/// The credential-store target names to try for `host`, in order.
///
/// `gh` stores its OAuth token under `gh:<host>:<user>`, and leaves a
/// user-less `gh:<host>:` entry beside it. Git Credential Manager stores the
/// git credential under `git:https://<host>`. Trying `gh`'s first prefers a
/// token minted for API use over one minted for pushing.
pub fn os_store_targets(host: &str, gh_username: Option<&str>) -> Vec<String> {
    let mut targets = Vec::new();
    if let Some(username) = gh_username.map(str::trim).filter(|user| !user.is_empty()) {
        targets.push(format!("gh:{host}:{username}"));
    }
    targets.push(format!("gh:{host}:"));
    targets.push(format!("git:https://{host}"));
    targets
}
