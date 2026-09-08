//! GitHub, over REST v3.
//!
//! Replaces `gh pr view` and `gh repo view`. The mapping is not quite
//! mechanical: `gh` reports a merged pull request as `MERGED`, while the REST
//! API reports it as `closed` with `merged_at` set. Downstream code understands
//! `MERGED` (`api_pull_request_state` in `ai/agent/api/convert_to.rs`), so the
//! REST shape is translated back into `gh`'s vocabulary rather than passed
//! through.

use async_trait::async_trait;

use crate::util::git::{PrInfo, RepositoryInfo};

use super::RepoRef;
use super::client::{ForgeClient, ForgeError, ForgeTransport, classify_status};

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;

/// GitHub's API root for a repository's host.
///
/// github.com is served from a separate domain; GitHub Enterprise serves the
/// same API under `/api/v3` on the host itself.
pub fn api_base_url(repo: &RepoRef) -> String {
    if repo.host == "github.com" || repo.host == "www.github.com" {
        return "https://api.github.com".to_owned();
    }
    let authority = super::detect::authority(&repo.host, repo.port);
    format!("https://{authority}/api/v3")
}

/// The pull request state in `gh`'s vocabulary.
///
/// REST reports a merged pull request as `closed`, distinguishing it only by
/// `merged_at`. Collapsing the two would report every merged pull request as
/// closed.
fn state_from_rest(state: &str, merged_at_is_set: bool) -> String {
    if merged_at_is_set {
        return "MERGED".to_owned();
    }
    state.to_ascii_uppercase()
}

/// Reads the first pull request out of a `GET /repos/{owner}/{repo}/pulls`
/// response body.
///
/// GitHub returns them newest first, and a branch with no pull request yields
/// an empty array — an answer, not an error.
pub fn parse_pr_list(body: &str) -> Result<Option<PrInfo>, ForgeError> {
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| ForgeError::Failed(format!("unreadable pull request list: {error}")))?;
    let Some(first) = parsed.as_array().and_then(|items| items.first()) else {
        return Ok(None);
    };

    let number = first["number"]
        .as_u64()
        .ok_or_else(|| ForgeError::Failed("pull request has no number".to_owned()))?;
    let url = first["html_url"]
        .as_str()
        .ok_or_else(|| ForgeError::Failed("pull request has no url".to_owned()))?
        .to_owned();
    let state = state_from_rest(
        first["state"].as_str().unwrap_or_default(),
        !first["merged_at"].is_null(),
    );
    let draft = first["draft"].as_bool().unwrap_or(false);
    let base_branch = first["base"]["ref"].as_str().unwrap_or_default().to_owned();

    Ok(Some(PrInfo {
        number,
        url,
        state,
        draft,
        base_branch,
    }))
}

/// Reads a `GET /repos/{owner}/{repo}` response body.
pub fn parse_repository(body: &str, host: &str) -> Result<RepositoryInfo, ForgeError> {
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| ForgeError::Failed(format!("unreadable repository: {error}")))?;
    let name = parsed["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ForgeError::Failed("repository has no name".to_owned()))?
        .to_owned();
    let owner = parsed["owner"]["login"]
        .as_str()
        .filter(|owner| !owner.is_empty())
        .map(str::to_owned);

    Ok(RepositoryInfo {
        name,
        owner,
        host: Some(host.to_owned()),
    })
}

/// A GitHub client for one host, holding the credential for that host and no
/// other.
pub struct GitHubClient {
    transport: Box<dyn ForgeTransport>,
    token: Option<String>,
}

impl GitHubClient {
    pub fn new(transport: Box<dyn ForgeTransport>, token: Option<String>) -> Self {
        Self { transport, token }
    }

    async fn get(&self, url: &str) -> Result<String, ForgeError> {
        let response = self
            .transport
            .get(url, self.token.as_deref())
            .await
            .map_err(|error| ForgeError::Failed(error.to_string()))?;
        if response.is_success() {
            return Ok(response.body);
        }
        Err(classify_status(
            response.status,
            self.token.is_some(),
            &response.body,
        ))
    }
}

#[async_trait]
impl ForgeClient for GitHubClient {
    async fn pr_for_branch(
        &self,
        repo: &RepoRef,
        branch: &str,
    ) -> Result<Option<PrInfo>, ForgeError> {
        // `head` is qualified with the owner so a fork's branch of the same
        // name does not match. `state=all` so a merged or closed pull request
        // is still reported, which is what `gh pr view` does.
        let url = format!(
            "{}/repos/{}/{}/pulls?head={}:{}&state=all&per_page=1",
            api_base_url(repo),
            repo.owner,
            repo.name,
            repo.owner,
            branch
        );
        parse_pr_list(&self.get(&url).await?)
    }

    async fn repository_info(&self, repo: &RepoRef) -> Result<RepositoryInfo, ForgeError> {
        let url = format!("{}/repos/{}/{}", api_base_url(repo), repo.owner, repo.name);
        parse_repository(&self.get(&url).await?, &repo.host)
    }
}
