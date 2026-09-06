use std::path::Path;

use git2::Repository;
use tempfile::TempDir;

use super::{
    branch_commit_summaries, committed_file_changes, current_branch_display,
    current_branch_name_or_head, local_branch_names, main_branch_ref,
};

/// A repository with one commit on `main`, built with git2 so the fixture
/// needs no `git` on `PATH`.
fn repo_with_one_commit() -> (TempDir, Repository) {
    let dir = TempDir::new().unwrap();
    let repository = Repository::init(dir.path()).unwrap();
    {
        let mut config = repository.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
    }
    std::fs::write(dir.path().join("file.txt"), "contents\n").unwrap();
    let tree_id = {
        let mut index = repository.index().unwrap();
        index.add_path(Path::new("file.txt")).unwrap();
        index.write().unwrap();
        index.write_tree().unwrap()
    };
    {
        let tree = repository.find_tree(tree_id).unwrap();
        let signature = repository.signature().unwrap();
        repository
            .commit(Some("HEAD"), &signature, &signature, "first", &tree, &[])
            .unwrap();
    }
    // `Repository::init` names the branch whatever `init.defaultBranch` says on
    // the machine running the test, so pin it rather than assume "main".
    // `set_head` accepts a ref that does not exist yet -- that is how an unborn
    // HEAD is expressed -- so the branch has to be created first.
    {
        let head_commit = repository.head().unwrap().peel_to_commit().unwrap();
        repository.branch("main", &head_commit, true).unwrap();
    }
    repository.set_head("refs/heads/main").unwrap();
    (dir, repository)
}

fn detach_head(repository: &Repository) {
    let head_oid = repository.head().unwrap().peel_to_commit().unwrap().id();
    repository.set_head_detached(head_oid).unwrap();
}

#[test]
fn current_branch_name_or_head_returns_the_branch_name() {
    let (_dir, repository) = repo_with_one_commit();
    assert_eq!(current_branch_name_or_head(&repository).unwrap(), "main");
}

/// `detect_current_branch` documents that a detached HEAD yields the literal
/// "HEAD", which is what distinguishes it from the `_display` variant.
#[test]
fn current_branch_name_or_head_returns_head_when_detached() {
    let (_dir, repository) = repo_with_one_commit();
    detach_head(&repository);
    assert_eq!(current_branch_name_or_head(&repository).unwrap(), "HEAD");
}

#[test]
fn current_branch_display_returns_the_branch_name() {
    let (_dir, repository) = repo_with_one_commit();
    assert_eq!(current_branch_display(&repository).unwrap(), "main");
}

/// The `_display` variant reports a short SHA where the plain one reports
/// "HEAD". This is the difference the two functions exist for.
#[test]
fn current_branch_display_returns_a_short_sha_when_detached() {
    let (_dir, repository) = repo_with_one_commit();
    let head_oid = repository.head().unwrap().peel_to_commit().unwrap().id();
    detach_head(&repository);

    let display = current_branch_display(&repository).unwrap();
    assert_ne!(
        display, "HEAD",
        "the display variant must not return the literal HEAD"
    );
    assert_eq!(
        display.len(),
        7,
        "expected a seven-character short SHA, got {display:?}"
    );
    assert!(
        head_oid.to_string().starts_with(&display),
        "{display:?} should be a prefix of {head_oid}"
    );
}

/// A repository with no commits has an unborn HEAD, which libgit2 reports as an
/// error rather than a branch. The branch name is still knowable from the
/// symbolic ref, and callers rely on getting it.
#[test]
fn current_branch_name_or_head_reads_an_unborn_branch() {
    let dir = TempDir::new().unwrap();
    let repository = Repository::init(dir.path()).unwrap();
    repository.set_head("refs/heads/main").unwrap();

    assert_eq!(current_branch_name_or_head(&repository).unwrap(), "main");
}

/// Commits `name` on the current branch and returns the new commit id.
fn commit_file(repository: &Repository, name: &str, contents: &str) -> git2::Oid {
    let workdir = repository.workdir().unwrap().to_owned();
    std::fs::write(workdir.join(name), contents).unwrap();
    let tree_id = {
        let mut index = repository.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        index.write_tree().unwrap()
    };
    let tree = repository.find_tree(tree_id).unwrap();
    let signature = repository.signature().unwrap();
    let parent = repository.head().unwrap().peel_to_commit().unwrap();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            &format!("add {name}"),
            &tree,
            &[&parent],
        )
        .unwrap()
}

/// Points `refs/remotes/origin/HEAD` at `refs/remotes/origin/<branch>`, the way
/// a clone does, and creates that remote-tracking ref.
fn set_origin_head(repository: &Repository, branch: &str) {
    let head_commit = repository.head().unwrap().peel_to_commit().unwrap();
    repository
        .reference(
            &format!("refs/remotes/origin/{branch}"),
            head_commit.id(),
            true,
            "test",
        )
        .unwrap();
    repository
        .reference_symbolic(
            "refs/remotes/origin/HEAD",
            &format!("refs/remotes/origin/{branch}"),
            true,
            "test",
        )
        .unwrap();
}

/// `detect_main_branch` reports remote-qualified names: `origin/HEAD` pointing
/// at `origin/main` yields "origin/main", not "main". Every downstream
/// `merge-base` and `base..HEAD` range depends on which of the two it gets.
#[test]
fn main_branch_ref_prefers_origin_head_and_keeps_the_remote_prefix() {
    let (_dir, repository) = repo_with_one_commit();
    set_origin_head(&repository, "main");

    assert_eq!(main_branch_ref(&repository), "origin/main");
}

/// Without `origin/HEAD` the candidate list decides, and it prefers the
/// remote-tracking ref over the local branch of the same name.
#[test]
fn main_branch_ref_falls_back_to_the_candidate_list() {
    let (_dir, repository) = repo_with_one_commit();
    assert_eq!(main_branch_ref(&repository), "main");

    let head_commit = repository.head().unwrap().peel_to_commit().unwrap();
    repository
        .reference("refs/remotes/origin/main", head_commit.id(), true, "test")
        .unwrap();
    assert_eq!(main_branch_ref(&repository), "origin/main");
}

/// The last resort is the current branch, and on a detached HEAD
/// `git branch --show-current` prints nothing at all.
#[test]
fn main_branch_ref_is_empty_on_a_detached_head_with_no_candidates() {
    let dir = TempDir::new().unwrap();
    let repository = Repository::init(dir.path()).unwrap();
    {
        let mut config = repository.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
    }
    std::fs::write(dir.path().join("file.txt"), "contents\n").unwrap();
    let tree_id = {
        let mut index = repository.index().unwrap();
        index.add_path(Path::new("file.txt")).unwrap();
        index.write().unwrap();
        index.write_tree().unwrap()
    };
    {
        let tree = repository.find_tree(tree_id).unwrap();
        let signature = repository.signature().unwrap();
        repository
            .commit(Some("HEAD"), &signature, &signature, "first", &tree, &[])
            .unwrap();
    }
    // Move off the default branch and delete it, so no candidate in
    // ["origin/main", "origin/master", "main", "master", "develop"] resolves.
    // `Repository::init` names that branch from `init.defaultBranch`, which
    // varies by machine, so read it rather than assume.
    let original_branch = {
        let head = repository.head().unwrap();
        head.shorthand().unwrap().to_owned()
    };
    let head_commit = repository.head().unwrap().peel_to_commit().unwrap();
    repository.branch("topic", &head_commit, true).unwrap();
    repository.set_head("refs/heads/topic").unwrap();
    if original_branch != "topic" {
        repository
            .find_branch(&original_branch, git2::BranchType::Local)
            .unwrap()
            .delete()
            .unwrap();
    }
    detach_head(&repository);

    assert_eq!(main_branch_ref(&repository), "");
}

#[test]
fn local_branch_names_lists_every_local_branch() {
    let (_dir, repository) = repo_with_one_commit();
    let head_commit = repository.head().unwrap().peel_to_commit().unwrap();
    repository.branch("feature", &head_commit, true).unwrap();
    repository
        .reference("refs/remotes/origin/main", head_commit.id(), true, "test")
        .unwrap();

    let names = local_branch_names(&repository);
    assert!(names.contains("main"), "expected main in {names:?}");
    assert!(names.contains("feature"), "expected feature in {names:?}");
    assert!(
        !names.iter().any(|name| name.starts_with("origin/")),
        "remote-tracking refs are not local branches: {names:?}"
    );
}

#[test]
fn branch_commit_summaries_lists_commits_after_the_base() {
    let (_dir, repository) = repo_with_one_commit();
    let base = repository.head().unwrap().peel_to_commit().unwrap();
    repository.branch("feature", &base, true).unwrap();
    repository.set_head("refs/heads/feature").unwrap();
    commit_file(&repository, "b.txt", "b\n");
    commit_file(&repository, "c.txt", "c\n");

    let summaries = branch_commit_summaries(&repository, "main").unwrap();
    assert_eq!(summaries, vec!["add c.txt", "add b.txt"]);
}

#[test]
fn branch_commit_summaries_is_empty_when_the_branch_has_no_commits_of_its_own() {
    let (_dir, repository) = repo_with_one_commit();
    let base = repository.head().unwrap().peel_to_commit().unwrap();
    repository.branch("feature", &base, true).unwrap();
    repository.set_head("refs/heads/feature").unwrap();

    assert!(
        branch_commit_summaries(&repository, "main")
            .unwrap()
            .is_empty(),
        "a branch level with its base has no commits of its own"
    );
}

#[test]
fn committed_file_changes_counts_added_and_deleted_lines() {
    let (_dir, repository) = repo_with_one_commit();
    let base = repository.head().unwrap().peel_to_commit().unwrap();
    repository.branch("feature", &base, true).unwrap();
    repository.set_head("refs/heads/feature").unwrap();
    commit_file(&repository, "b.txt", "one\ntwo\nthree\n");

    let entries = committed_file_changes(&repository, "main").unwrap();
    assert_eq!(
        entries.len(),
        1,
        "expected one changed file, got {entries:?}"
    );
    assert_eq!(entries[0].0, "b.txt");
    assert_eq!(entries[0].1, 3, "three added lines");
    assert_eq!(entries[0].2, 0, "no deleted lines");
}

/// `git diff --numstat` renders a binary file as `-\t-\t<path>`, and the shell
/// implementation turned those unparsable counts into zeroes. The native path
/// must report the file with zero counts too, not drop it.
#[test]
fn committed_file_changes_reports_a_binary_file_with_zero_counts() {
    let (_dir, repository) = repo_with_one_commit();
    let base = repository.head().unwrap().peel_to_commit().unwrap();
    repository.branch("feature", &base, true).unwrap();
    repository.set_head("refs/heads/feature").unwrap();
    commit_file(&repository, "blob.bin", "\0\u{1}\u{2}binary\0content\u{3}");

    let entries = committed_file_changes(&repository, "main").unwrap();
    assert_eq!(
        entries.len(),
        1,
        "the binary file must still be listed: {entries:?}"
    );
    assert_eq!(entries[0].0, "blob.bin");
    assert_eq!(entries[0].1, 0, "a binary file reports no added lines");
    assert_eq!(entries[0].2, 0, "a binary file reports no deleted lines");
}

/// Uncommitted and untracked files are excluded: the range is
/// `<merge-base>..HEAD`, not the working tree.
#[test]
fn committed_file_changes_excludes_the_working_tree() {
    let (_dir, repository) = repo_with_one_commit();
    let base = repository.head().unwrap().peel_to_commit().unwrap();
    repository.branch("feature", &base, true).unwrap();
    repository.set_head("refs/heads/feature").unwrap();
    commit_file(&repository, "b.txt", "one\n");

    let workdir = repository.workdir().unwrap().to_owned();
    std::fs::write(workdir.join("untracked.txt"), "nope\n").unwrap();
    std::fs::write(workdir.join("b.txt"), "one\nedited\n").unwrap();

    let entries = committed_file_changes(&repository, "main").unwrap();
    assert_eq!(
        entries.len(),
        1,
        "only the committed change counts: {entries:?}"
    );
    assert_eq!(entries[0].0, "b.txt");
    assert_eq!(
        entries[0].1, 1,
        "the uncommitted second line must not be counted"
    );
}

/// A branch with no merge base against `main` yields nothing rather than
/// failing, matching the shell implementation's early return.
#[test]
fn committed_file_changes_is_empty_without_a_merge_base() {
    let (_dir, repository) = repo_with_one_commit();
    assert!(
        committed_file_changes(&repository, "no-such-branch")
            .unwrap()
            .is_empty()
    );
}

/// `git diff --numstat` renders a rename as a single row whose path column is
/// the combined string "old => new", which is not a path any consumer can use.
/// Rename detection stays on, matching git's `diff.renames` default, but the
/// entry carries the new path alone. This is a deliberate divergence from the
/// shell implementation, which passed numstat's combined string through.
#[test]
fn committed_file_changes_reports_a_rename_under_its_new_path() {
    let (_dir, repository) = repo_with_one_commit();
    let base = repository.head().unwrap().peel_to_commit().unwrap();
    repository.branch("feature", &base, true).unwrap();
    repository.set_head("refs/heads/feature").unwrap();

    let workdir = repository.workdir().unwrap().to_owned();
    std::fs::remove_file(workdir.join("file.txt")).unwrap();
    std::fs::write(workdir.join("renamed.txt"), "contents\n").unwrap();
    let tree_id = {
        let mut index = repository.index().unwrap();
        index.remove_path(Path::new("file.txt")).unwrap();
        index.add_path(Path::new("renamed.txt")).unwrap();
        index.write().unwrap();
        index.write_tree().unwrap()
    };
    {
        let tree = repository.find_tree(tree_id).unwrap();
        let signature = repository.signature().unwrap();
        let parent = repository.head().unwrap().peel_to_commit().unwrap();
        repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "rename file.txt",
                &tree,
                &[&parent],
            )
            .unwrap();
    }

    let entries = committed_file_changes(&repository, "main").unwrap();
    assert_eq!(
        entries.len(),
        1,
        "a rename is one entry, not a delete plus an add: {entries:?}"
    );
    assert_eq!(
        entries[0].0, "renamed.txt",
        "the entry must carry a usable path, not numstat's \"old => new\""
    );
}
