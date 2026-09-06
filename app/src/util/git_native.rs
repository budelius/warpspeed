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

/// The repository's default branch as a *ref name*, remote prefix intact.
///
/// Matches [`crate::util::git::detect_main_branch`], which reports
/// `origin/main` rather than `main` — the distinction every downstream
/// `merge-base` and `base..HEAD` range depends on. This is deliberately not
/// [`main_branch_name`], which strips the remote and searches local branches
/// only.
///
/// Falls back to the current branch, and to an empty string on a detached HEAD,
/// mirroring `git branch --show-current`, which prints nothing when detached.
pub fn main_branch_ref(repository: &Repository) -> String {
    if let Ok(origin_head) = repository.find_reference("refs/remotes/origin/HEAD")
        && let Some(branch) = origin_head
            .symbolic_target()
            .and_then(|target| target.strip_prefix("refs/remotes/"))
    {
        return branch.to_owned();
    }

    let candidate = ["origin/main", "origin/master", "main", "master", "develop"]
        .into_iter()
        .find(|candidate| {
            repository
                .revparse_single(candidate)
                .and_then(|object| object.peel_to_commit())
                .is_ok()
        });
    if let Some(candidate) = candidate {
        return candidate.to_owned();
    }

    match repository.head() {
        Ok(head) if head.is_branch() => head.shorthand().unwrap_or_default().to_owned(),
        _ => String::new(),
    }
}

/// Every local branch name. Remote-tracking refs are not included.
///
/// Matches `git branch --list --format=%(refname:short)`, and keeps that
/// helper's contract of yielding an empty set rather than failing.
pub fn local_branch_names(repository: &Repository) -> std::collections::HashSet<String> {
    let Ok(branches) = repository.branches(Some(BranchType::Local)) else {
        return std::collections::HashSet::new();
    };
    branches
        .filter_map(|branch| branch.ok())
        .filter_map(|(branch, _)| branch.name().ok().flatten().map(str::to_owned))
        .collect()
}

/// Commit subject lines on HEAD that `base` does not have, newest first.
///
/// Matches `git log <base>..HEAD --format=%s`.
pub fn branch_commit_summaries(repository: &Repository, base: &str) -> anyhow::Result<Vec<String>> {
    let base_commit = repository
        .revparse_single(base)
        .and_then(|object| object.peel_to_commit())
        .with_context(|| format!("Unable to resolve base ref {base}"))?;

    let mut revwalk = repository.revwalk()?;
    revwalk.set_sorting(git2::Sort::TIME)?;
    revwalk.push_head()?;
    revwalk.hide(base_commit.id())?;

    let mut summaries = Vec::new();
    for oid in revwalk {
        let commit = repository.find_commit(oid?)?;
        if let Some(summary) = commit.summary() {
            summaries.push(summary.to_owned());
        }
    }
    Ok(summaries)
}

/// Per-file line counts for the commits on HEAD that `main_branch` does not
/// have: the committed-only diff, equivalent to `git diff --numstat
/// $(git merge-base HEAD <main>) HEAD`.
///
/// Returns `(path, additions, deletions)`. A binary file is reported with zero
/// counts rather than dropped, matching how the shell implementation turned
/// numstat's `-\t-` into zeroes. No merge base yields an empty vector rather
/// than an error, which is also what the shell implementation did.
pub fn committed_file_changes(
    repository: &Repository,
    main_branch: &str,
) -> anyhow::Result<Vec<(String, usize, usize)>> {
    let Ok(main_commit) = repository
        .revparse_single(main_branch)
        .and_then(|object| object.peel_to_commit())
    else {
        return Ok(Vec::new());
    };
    let head_commit = repository.head()?.peel_to_commit()?;
    let Ok(merge_base) = repository.merge_base(head_commit.id(), main_commit.id()) else {
        return Ok(Vec::new());
    };

    let base_tree = repository.find_commit(merge_base)?.tree()?;
    let head_tree = head_commit.tree()?;
    let mut diff = repository.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), None)?;
    // `diff.renames` defaults to true in git, so the shell implementation saw
    // rename detection on. Match it, but report the new path rather than
    // numstat's combined "old => new" string, which is not a usable path.
    diff.find_similar(Some(git2::DiffFindOptions::new().renames(true)))?;

    let mut entries = Vec::new();
    for index in 0..diff.deltas().len() {
        let delta = diff.get_delta(index).context("Missing diff delta")?;
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            // git stores paths with forward slashes on every platform, so
            // `to_string_lossy` needs no separator conversion here.
            .map(|path| path.to_string_lossy().into_owned())
            .context("Diff delta has no path")?;

        // A binary delta has no patch to count lines from; report it at zero.
        let (additions, deletions) = match git2::Patch::from_diff(&diff, index)? {
            Some(patch) => {
                let (_context, additions, deletions) = patch.line_stats()?;
                (additions, deletions)
            }
            None => (0, 0),
        };
        entries.push((path, additions, deletions));
    }
    Ok(entries)
}

/// The patch text between `base` and `end_ref`, as `git diff <base>..<end_ref>`
/// would render it.
///
/// Both refs are resolved and peeled to trees, so this is the commit-to-commit
/// diff and never consults the working tree.
pub fn diff_text_between(
    repository: &Repository,
    base: &str,
    end_ref: &str,
) -> anyhow::Result<String> {
    let base_tree = repository
        .revparse_single(base)
        .and_then(|object| object.peel_to_commit())
        .with_context(|| format!("Unable to resolve base ref {base}"))?
        .tree()?;
    let end_tree = repository
        .revparse_single(end_ref)
        .and_then(|object| object.peel_to_commit())
        .with_context(|| format!("Unable to resolve ref {end_ref}"))?
        .tree()?;

    let diff = repository.diff_tree_to_tree(Some(&base_tree), Some(&end_tree), None)?;
    let mut text = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        match line.origin() {
            '+' | '-' | ' ' => text.push(line.origin()),
            _ => {}
        }
        text.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    Ok(text)
}

/// Whether `reference` resolves to a commit in this repository.
///
/// Matches `git rev-parse --verify <ref>`, which is how callers decide between
/// a remote-tracking ref and `HEAD`.
pub fn resolves_to_commit(repository: &Repository, reference: &str) -> bool {
    repository
        .revparse_single(reference)
        .and_then(|object| object.peel_to_commit())
        .is_ok()
}
