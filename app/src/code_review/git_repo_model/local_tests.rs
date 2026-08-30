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
