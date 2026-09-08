use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::executor::block_on;

use super::{api_base_url, parse_pr_list, parse_repository};
use crate::forge::RepoRef;
use crate::forge::client::{
    ForgeClient, ForgeError, ForgeTransport, HttpResponse, classify_status,
};
use crate::forge::github::GitHubClient;

fn repo(host: &str) -> RepoRef {
    RepoRef {
        host: host.to_owned(),
        port: None,
        owner: "warpdotdev".to_owned(),
        name: "Warp".to_owned(),
    }
}

/// Every `(url, token)` a stub was asked with, so a test can assert on the
/// request and not only on the parsed answer.
type SeenRequests = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Returns one canned response and records the URL and token it was asked with.
struct StubTransport {
    response: HttpResponse,
    seen: SeenRequests,
}

impl StubTransport {
    fn new(status: u16, body: &str) -> (Box<Self>, SeenRequests) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Box::new(Self {
                response: HttpResponse {
                    status,
                    body: body.to_owned(),
                },
                seen: seen.clone(),
            }),
            seen,
        )
    }
}

#[async_trait]
impl ForgeTransport for StubTransport {
    async fn get(&self, url: &str, token: Option<&str>) -> anyhow::Result<HttpResponse> {
        self.seen
            .lock()
            .unwrap()
            .push((url.to_owned(), token.map(str::to_owned)));
        Ok(self.response.clone())
    }
}

const OPEN_PR: &str = r#"[{
    "number": 42,
    "html_url": "https://github.com/warpdotdev/Warp/pull/42",
    "state": "open",
    "merged_at": null,
    "draft": false,
    "base": { "ref": "main" }
}]"#;

#[test]
fn api_base_url_uses_the_api_subdomain_for_github_com() {
    assert_eq!(api_base_url(&repo("github.com")), "https://api.github.com");
}

/// GitHub Enterprise serves the same API from the host itself, under `/api/v3`.
#[test]
fn api_base_url_uses_the_host_for_github_enterprise() {
    assert_eq!(
        api_base_url(&repo("git.example.com")),
        "https://git.example.com/api/v3"
    );
    let mut with_port = repo("git.example.com");
    with_port.port = Some(8443);
    assert_eq!(
        api_base_url(&with_port),
        "https://git.example.com:8443/api/v3"
    );
}

#[test]
fn parse_pr_list_reads_an_open_pull_request() {
    let pr = parse_pr_list(OPEN_PR)
        .unwrap()
        .expect("expected a pull request");
    assert_eq!(pr.number, 42);
    assert_eq!(pr.url, "https://github.com/warpdotdev/Warp/pull/42");
    assert_eq!(pr.state, "OPEN");
    assert!(!pr.draft);
    assert_eq!(pr.base_branch, "main");
}

/// The divergence that matters. REST reports a merged pull request as `closed`
/// with `merged_at` set, while `gh` reports `MERGED`, and downstream code
/// (`api_pull_request_state`) understands the latter. Passing `closed` through
/// would report every merged pull request as merely closed.
#[test]
fn parse_pr_list_reports_a_merged_pull_request_as_merged() {
    let body = r#"[{
        "number": 7,
        "html_url": "https://github.com/o/r/pull/7",
        "state": "closed",
        "merged_at": "2026-09-01T10:00:00Z",
        "draft": false,
        "base": { "ref": "main" }
    }]"#;
    let pr = parse_pr_list(body).unwrap().unwrap();
    assert_eq!(pr.state, "MERGED", "a merged PR must not read as CLOSED");
}

#[test]
fn parse_pr_list_reports_a_closed_pull_request_as_closed() {
    let body = r#"[{
        "number": 8,
        "html_url": "https://github.com/o/r/pull/8",
        "state": "closed",
        "merged_at": null,
        "draft": false,
        "base": { "ref": "main" }
    }]"#;
    assert_eq!(parse_pr_list(body).unwrap().unwrap().state, "CLOSED");
}

#[test]
fn parse_pr_list_carries_the_draft_flag() {
    let body = r#"[{
        "number": 9,
        "html_url": "https://github.com/o/r/pull/9",
        "state": "open",
        "merged_at": null,
        "draft": true,
        "base": { "ref": "dev" }
    }]"#;
    let pr = parse_pr_list(body).unwrap().unwrap();
    assert!(pr.draft);
    assert_eq!(pr.base_branch, "dev");
}

/// A branch with no pull request is an answer, not a failure.
#[test]
fn parse_pr_list_reports_no_pull_request_for_an_empty_array() {
    assert!(parse_pr_list("[]").unwrap().is_none());
}

#[test]
fn parse_pr_list_fails_on_an_unreadable_body() {
    assert!(matches!(
        parse_pr_list("not json"),
        Err(ForgeError::Failed(_))
    ));
}

#[test]
fn parse_repository_reads_the_name_and_owner() {
    let body = r#"{ "name": "Warp", "owner": { "login": "warpdotdev" } }"#;
    let info = parse_repository(body, "github.com").unwrap();
    assert_eq!(info.name, "Warp");
    assert_eq!(info.owner.as_deref(), Some("warpdotdev"));
    assert_eq!(info.host.as_deref(), Some("github.com"));
}

#[test]
fn parse_repository_fails_without_a_name() {
    let body = r#"{ "owner": { "login": "warpdotdev" } }"#;
    assert!(matches!(
        parse_repository(body, "github.com"),
        Err(ForgeError::Failed(_))
    ));
}

/// 401 and 403 mean the token is missing, expired, or unscoped. Both are
/// "sign in", not "something broke".
#[test]
fn classify_status_treats_401_and_403_as_unauthenticated() {
    assert_eq!(classify_status(401, true, ""), ForgeError::Unauthenticated);
    assert_eq!(classify_status(403, true, ""), ForgeError::Unauthenticated);
}

/// A forge hides a private repository's existence from an unauthorised caller,
/// so an unauthenticated 404 means "you may not be signed in", while an
/// authenticated one is a real failure.
#[test]
fn classify_status_reads_404_differently_with_and_without_a_token() {
    assert_eq!(
        classify_status(404, false, ""),
        ForgeError::Unauthenticated,
        "without a token a 404 may just be a private repository"
    );
    assert!(matches!(
        classify_status(404, true, ""),
        ForgeError::Failed(_)
    ));
}

#[test]
fn classify_status_truncates_a_long_body() {
    let body = "x".repeat(1000);
    let ForgeError::Failed(message) = classify_status(500, true, &body) else {
        panic!("expected a failure");
    };
    assert!(message.chars().count() < 300, "body should be truncated");
}

#[test]
fn pr_for_branch_qualifies_the_head_with_the_owner() {
    let (transport, seen) = StubTransport::new(200, OPEN_PR);
    let client = GitHubClient::new(transport, Some("token".to_owned()));

    let pr = block_on(client.pr_for_branch(&repo("github.com"), "feature/x")).unwrap();
    assert_eq!(pr.unwrap().number, 42);

    let seen = seen.lock().unwrap();
    let (url, token) = &seen[0];
    assert!(
        url.contains("head=warpdotdev:feature/x"),
        "the head must be owner-qualified so a fork's branch does not match: {url}"
    );
    assert!(
        url.contains("state=all"),
        "a merged or closed PR must still be found, as gh pr view does: {url}"
    );
    assert_eq!(token.as_deref(), Some("token"));
}

#[test]
fn a_401_from_the_transport_surfaces_as_unauthenticated() {
    let (transport, _) = StubTransport::new(401, "Bad credentials");
    let client = GitHubClient::new(transport, Some("token".to_owned()));

    assert_eq!(
        block_on(client.pr_for_branch(&repo("github.com"), "main")).unwrap_err(),
        ForgeError::Unauthenticated
    );
}

#[test]
fn a_server_error_surfaces_as_a_failure() {
    let (transport, _) = StubTransport::new(500, "boom");
    let client = GitHubClient::new(transport, Some("token".to_owned()));

    assert!(matches!(
        block_on(client.repository_info(&repo("github.com"))).unwrap_err(),
        ForgeError::Failed(_)
    ));
}
