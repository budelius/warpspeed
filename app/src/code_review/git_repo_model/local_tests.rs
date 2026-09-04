use std::path::{Path, PathBuf};

use repo_metadata::{RepositoryUpdate, TargetFile};

use super::*;

#[test]
fn should_refresh_metadata_ignores_ignored_file_updates() {
    let mut ignored_update = RepositoryUpdate::default();
    ignored_update
        .modified
        .insert(TargetFile::new(PathBuf::from("/repo/ignored.log"), true));
    assert!(!LocalGitRepoStatusModel::should_refresh_metadata(
        &ignored_update
    ));

    let mut tracked_update = RepositoryUpdate::default();
    tracked_update
        .modified
        .insert(TargetFile::new(PathBuf::from("/repo/src/main.rs"), false));
    assert!(LocalGitRepoStatusModel::should_refresh_metadata(
        &tracked_update
    ));

    let remote_ref_update = RepositoryUpdate {
        remote_ref_updated: true,
        ..Default::default()
    };
    assert!(LocalGitRepoStatusModel::should_refresh_metadata(
        &remote_ref_update
    ));
}

#[test]
fn native_git_metadata_supports_unborn_branch() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let repository = GitRepository::init(temp_dir.path()).unwrap();
    std::fs::write(temp_dir.path().join("untracked.txt"), "new\n").unwrap();

    let current_branch = LocalGitRepoStatusModel::current_branch_name(&repository).unwrap();
    let stats = LocalGitRepoStatusModel::diff_stats(&repository).unwrap();

    assert!(!current_branch.is_empty());
    assert_eq!(stats.files_changed, 1);
    assert_eq!(stats.total_additions, 1);
    assert_eq!(stats.total_deletions, 0);
}

#[test]
fn native_git_metadata_reads_branch_and_worktree_diff() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let repository = GitRepository::init(temp_dir.path()).unwrap();
    std::fs::write(temp_dir.path().join("tracked.txt"), "one\n").unwrap();
    let mut index = repository.index().unwrap();
    index.add_path(Path::new("tracked.txt")).unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("Warp", "warp@example.com").unwrap();
    repository
        .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
        .unwrap();
    std::fs::write(temp_dir.path().join("tracked.txt"), "one\ntwo\n").unwrap();
    std::fs::write(temp_dir.path().join("untracked.txt"), "new\n").unwrap();

    let current_branch = LocalGitRepoStatusModel::current_branch_name(&repository).unwrap();
    let main_branch = LocalGitRepoStatusModel::main_branch_name(&repository, &current_branch);
    let stats = LocalGitRepoStatusModel::diff_stats(&repository).unwrap();

    assert!(!current_branch.is_empty());
    assert_eq!(main_branch, current_branch);
    assert_eq!(stats.files_changed, 2);
    assert_eq!(stats.total_additions, 2);
    assert_eq!(stats.total_deletions, 0);
}

/// Commit an empty-ish change at a fixed timestamp so branch ordering in
/// `branch_dropdown_lines` is deterministic rather than wall-clock dependent.
fn commit_at(repository: &GitRepository, file: &str, seconds: i64) -> git2::Oid {
    let workdir = repository.workdir().unwrap().to_path_buf();
    std::fs::write(workdir.join(file), format!("{file}\n")).unwrap();
    let mut index = repository.index().unwrap();
    index.add_path(Path::new(file)).unwrap();
    let tree_id = index.write_tree().unwrap();
    index.write().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    let time = git2::Time::new(seconds, 0);
    let signature = git2::Signature::new("Warp", "warp@example.com", &time).unwrap();
    let parents = match repository.head().ok().and_then(|head| head.target()) {
        Some(oid) => vec![repository.find_commit(oid).unwrap()],
        None => Vec::new(),
    };
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            file,
            &tree,
            &parent_refs,
        )
        .unwrap()
}

#[test]
fn branch_dropdown_lines_match_the_git_branch_command_shape() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let repository = GitRepository::init(temp_dir.path()).unwrap();

    let oldest = commit_at(&repository, "a.txt", 1_000);
    repository
        .branch("oldest", &repository.find_commit(oldest).unwrap(), false)
        .unwrap();
    let middle = commit_at(&repository, "b.txt", 2_000);
    repository
        .branch("middle", &repository.find_commit(middle).unwrap(), false)
        .unwrap();
    commit_at(&repository, "c.txt", 3_000);

    let head_branch = LocalGitRepoStatusModel::current_branch_name(&repository).unwrap();
    let lines = LocalGitRepoStatusModel::branch_dropdown_lines(&repository).unwrap();

    let separator_index = lines
        .iter()
        .position(|line| line == WORKTREE_LIST_SEPARATOR)
        .expect("worktree separator should be emitted");
    let branch_lines = &lines[..separator_index];

    // Newest committer date first, exactly as `--sort=-committerdate` gave us.
    assert_eq!(
        branch_lines,
        [
            format!("* {head_branch}"),
            "  middle".to_string(),
            "  oldest".to_string(),
        ]
    );

    // And the existing on-click parser still understands the shape, keeping the
    // checked-out branch at the front.
    let filtered = crate::context_chips::git_branch_on_click::filter_git_branch_on_click_values(
        Some(lines),
    )
    .unwrap();
    assert_eq!(filtered, [head_branch, "middle".into(), "oldest".into()]);
}

#[test]
fn branch_dropdown_lines_mark_branches_checked_out_in_linked_worktrees() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let repository = GitRepository::init(temp_dir.path()).unwrap();
    commit_at(&repository, "a.txt", 1_000);

    let worktree_dir = temp_dir.path().join("linked");
    repository
        .worktree("linked", &worktree_dir, None)
        .expect("creating a linked worktree should succeed");

    let lines = LocalGitRepoStatusModel::branch_dropdown_lines(&repository).unwrap();
    let separator_index = lines
        .iter()
        .position(|line| line == WORKTREE_LIST_SEPARATOR)
        .unwrap();
    let (branch_lines, worktree_lines) = lines.split_at(separator_index);

    assert!(
        branch_lines.iter().any(|line| line == "+ linked"),
        "branch checked out in a linked worktree should be marked with '+': {branch_lines:?}"
    );
    assert!(
        worktree_lines
            .iter()
            .any(|line| line == "branch refs/heads/linked"),
        "porcelain worktree section should name the branch: {worktree_lines:?}"
    );

    // The '+' branch resolves to its worktree path through the existing parser.
    let filtered = crate::context_chips::git_branch_on_click::filter_git_branch_on_click_values(
        Some(lines),
    )
    .unwrap();
    assert!(
        filtered
            .iter()
            .any(|value| value.starts_with("linked\u{1f}worktree\u{1f}")),
        "linked worktree branch should encode its path: {filtered:?}"
    );
}
