//! The seam every forge client sits behind.
//!
//! `ForgeTransport` exists so a client can be tested against recorded responses
//! rather than a network, the same role `ForgeProber` plays for detection.

use async_trait::async_trait;

use super::RepoRef;
use crate::util::git::{PrInfo, RepositoryInfo};

/// A response from a forge's API.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Performs the request. One method, because every forge lookup this layer
/// needs is a GET; PR creation adds a POST in its own stage.
#[async_trait]
pub trait ForgeTransport: Send + Sync {
    async fn get(&self, url: &str, token: Option<&str>) -> anyhow::Result<HttpResponse>;
}

/// Why a forge lookup did not produce an answer.
///
/// These are three different situations and the UI answers them differently, so
/// they must not collapse into one error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeError {
    /// No credential resolved for the host. Offer sign-in; do not report a
    /// failure.
    Unauthenticated,
    /// The host answered no probe, or answered one this build cannot speak to.
    /// Say so once and stop asking.
    Unsupported,
    /// The API or the network failed. Worth surfacing, and worth retrying.
    Failed(String),
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthenticated => write!(f, "not signed in to this forge"),
            Self::Unsupported => write!(f, "this host is not a supported forge"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ForgeError {}

/// Reads pull request and repository information from one forge.
#[async_trait]
pub trait ForgeClient: Send + Sync {
    /// The pull request for `branch`, or `None` when the branch has none.
    ///
    /// "No pull request" is an answer, not an error.
    async fn pr_for_branch(
        &self,
        repo: &RepoRef,
        branch: &str,
    ) -> Result<Option<PrInfo>, ForgeError>;

    /// The repository's name and owner as the forge reports them.
    async fn repository_info(&self, repo: &RepoRef) -> Result<RepositoryInfo, ForgeError>;
}

/// Turns a non-success status into the right kind of error.
///
/// 401 and 403 are the unauthenticated case: the token is missing, expired, or
/// lacks the scope. 404 is ambiguous on a private repository — the forge hides
/// existence from an unauthorised caller — so it is reported as unauthenticated
/// when no token was sent, and as a failure when one was.
pub fn classify_status(status: u16, had_token: bool, body: &str) -> ForgeError {
    match status {
        401 | 403 => ForgeError::Unauthenticated,
        404 if !had_token => ForgeError::Unauthenticated,
        _ => ForgeError::Failed(format!("forge returned {status}: {}", truncate(body, 200))),
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    text.chars().take(limit).collect::<String>() + "…"
}
