use std::path::Path;

use git2::Repository;
use tempfile::TempDir;

use super::{current_branch_display, current_branch_name_or_head};

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
