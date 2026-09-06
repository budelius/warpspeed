//! Repository questions answered from an in-process libgit2 handle rather than
//! by spawning `git`.
//!
//! Every function here takes an open [`git2::Repository`] and is synchronous.
//! Callers that live on the foreground executor wrap them in
//! `blocking::unblock`, one call per public helper, so a helper answering
//! several questions opens the repository once and holds it for all of them.

use anyhow::Context as _;
use git2::{BranchType, ErrorCode, Repository};

#[cfg(test)]
#[path = "git_native_tests.rs"]
mod tests;

/// The branch name at HEAD, or the literal `"HEAD"` when detached.
///
/// Matches `git rev-parse --abbrev-ref HEAD`, which is what
/// [`crate::util::git::detect_current_branch`] promises. Use
/// [`current_branch_display`] where a detached HEAD should read as a commit
/// instead.
pub fn current_branch_name_or_head(repository: &Repository) -> anyhow::Result<String> {
    match repository.head() {
        Ok(head) if head.is_branch() => head
            .shorthand()
            .map(str::to_owned)
            .context("Git HEAD branch name is not valid UTF-8"),
        Ok(_) => Ok("HEAD".to_owned()),
        Err(error) if error.code() == ErrorCode::UnbornBranch => unborn_branch_name(repository),
        Err(error) => Err(error.into()),
    }
}

/// The branch name at HEAD, or a short commit SHA when detached.
///
/// Matches `git symbolic-ref --short HEAD || git rev-parse --short HEAD`, which
/// is what [`crate::util::git::detect_current_branch_display`] promises.
pub fn current_branch_display(repository: &Repository) -> anyhow::Result<String> {
    match repository.head() {
        Ok(head) if head.is_branch() => head
            .shorthand()
            .map(str::to_owned)
            .context("Git HEAD branch name is not valid UTF-8"),
        Ok(head) => head
            .target()
            .map(|oid| oid.to_string().chars().take(7).collect())
            .context("Detached Git HEAD has no target"),
        Err(error) if error.code() == ErrorCode::UnbornBranch => unborn_branch_name(repository),
        Err(error) => Err(error.into()),
    }
}

/// The branch a repository with no commits is pointed at. libgit2 reports an
/// unborn HEAD as an error, but the symbolic ref still names the branch.
fn unborn_branch_name(repository: &Repository) -> anyhow::Result<String> {
    repository
        .find_reference("HEAD")?
        .symbolic_target()
        .and_then(|target| target.strip_prefix("refs/heads/"))
        .map(str::to_owned)
        .context("Unborn Git HEAD has no branch name")
}

/// The repository's default branch: `origin/HEAD` when it is set, then the
/// first of `main`, `master`, `trunk`, `develop` that exists locally, and
/// finally `current_branch_name` when none of them do.
pub fn main_branch_name(repository: &Repository, current_branch_name: &str) -> String {
    if let Ok(origin_head) = repository.find_reference("refs/remotes/origin/HEAD")
        && let Some(branch) = origin_head
            .symbolic_target()
            .and_then(|target| target.strip_prefix("refs/remotes/origin/"))
    {
        return branch.to_owned();
    }

    ["main", "master", "trunk", "develop"]
        .into_iter()
        .find(|candidate| repository.find_branch(candidate, BranchType::Local).is_ok())
        .unwrap_or(current_branch_name)
        .to_owned()
}
