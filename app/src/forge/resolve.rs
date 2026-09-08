//! Turning a repository path into a forge client that can answer for it.
//!
//! This is what `util::git`'s `gh` call sites consult before shelling out. Every
//! failure here is deliberately recoverable: the caller falls back to `gh`, so a
//! repository on a forge this layer cannot yet speak to behaves exactly as it
//! did before.

use std::path::Path;

use super::client::{ForgeClient, ForgeError};
use super::credentials::{CredentialChain, CredentialStore};
use super::github::{GitHubClient, api_base_url};
use super::os_credentials::OsCredentialStore;
use super::transport::{HttpForgeTransport, authority_of};
use super::{ForgeKind, RepoRef};
use crate::util::git::{PrInfo, RepositoryInfo};
use crate::util::git_native;

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;

/// The remote whose forge a repository is judged by, in order of preference.
///
/// A repository can have remotes on two different forges — this one has
/// `origin` on GitHub and `gitlab` on GitLab — so the choice has to be stated
/// rather than left to whichever remote git lists first.
const REMOTE_PREFERENCE: [&str; 3] = ["origin", "upstream", "gitlab"];

/// Picks the remote to treat as authoritative, given the names a repository has.
///
/// Falls back to the single remote when there is exactly one, and to nothing at
/// all when a repository has several unrecognised remotes — guessing there
/// would mean querying the wrong forge.
pub fn preferred_remote(names: &[String]) -> Option<&str> {
    for candidate in REMOTE_PREFERENCE {
        if let Some(found) = names.iter().find(|name| name.as_str() == candidate) {
            return Some(found.as_str());
        }
    }
    match names {
        [only] => Some(only.as_str()),
        _ => None,
    }
}

/// The repository reference and current branch for `repo_path`.
async fn repo_ref_and_branch(repo_path: &Path) -> Result<(RepoRef, String), ForgeError> {
    let repo_path = repo_path.to_owned();
    blocking::unblock(move || {
        let repository = git2::Repository::open(&repo_path)
            .map_err(|error| ForgeError::Failed(format!("not a git repository: {error}")))?;

        let names: Vec<String> = repository
            .remotes()
            .map_err(|error| ForgeError::Failed(error.to_string()))?
            .iter()
            .flatten()
            .map(str::to_owned)
            .collect();
        let remote_name = preferred_remote(&names).ok_or(ForgeError::Unsupported)?;
        let remote = repository
            .find_remote(remote_name)
            .map_err(|error| ForgeError::Failed(error.to_string()))?;
        let url = remote.url().ok_or(ForgeError::Unsupported)?;
        let repo_ref = RepoRef::from_remote_url(url).ok_or(ForgeError::Unsupported)?;

        let branch = git_native::current_branch_name_or_head(&repository)
            .map_err(|error| ForgeError::Failed(error.to_string()))?;
        Ok((repo_ref, branch))
    })
    .await
}

/// Builds a client for a repository, or explains why there is none.
///
/// Detection is deliberately limited to hosts knowable by name. Probing an
/// unknown host sends an unauthenticated request the user did not ask for, and
/// no client exists for GitLab or Gitea yet, so there is nothing to gain by it.
fn client_for(repo: &RepoRef) -> Result<Box<dyn ForgeClient>, ForgeError> {
    match ForgeKind::for_known_host(&repo.host) {
        Some(ForgeKind::GitHub) => {}
        // GitLab and Gitea clients are not built yet; `gh` remains the path,
        // which for those hosts means the caller gets what it got before.
        Some(_) | None => return Err(ForgeError::Unsupported),
    }

    let base_url = api_base_url(repo);
    let api_authority = authority_of(&base_url).ok_or(ForgeError::Unsupported)?;

    // The credential is stored under the repository's host, but it travels to
    // the API host, which for github.com is a different name.
    let stores: Vec<Box<dyn CredentialStore>> = vec![Box::new(OsCredentialStore)];
    let token = CredentialChain::new(stores)
        .resolve(&repo.host)
        .map(|credential| credential.token);

    let transport = HttpForgeTransport::new(http_client::Client::new(), api_authority);
    Ok(Box::new(GitHubClient::new(Box::new(transport), token)))
}

/// The pull request for the repository's current branch, from its forge's API.
pub async fn pr_for_branch(repo_path: &Path) -> Result<Option<PrInfo>, ForgeError> {
    let (repo, branch) = repo_ref_and_branch(repo_path).await?;
    // A detached HEAD has no branch to look a pull request up by.
    if branch == "HEAD" {
        return Ok(None);
    }
    client_for(&repo)?.pr_for_branch(&repo, &branch).await
}

/// The repository's name and owner, from its forge's API.
pub async fn repository_info(repo_path: &Path) -> Result<RepositoryInfo, ForgeError> {
    let (repo, _branch) = repo_ref_and_branch(repo_path).await?;
    client_for(&repo)?.repository_info(&repo).await
}
