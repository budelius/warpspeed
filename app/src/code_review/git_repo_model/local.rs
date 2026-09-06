use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use async_channel::Sender;
use git2::{BranchType, DiffOptions, ErrorCode, Repository as GitRepository};
use repo_metadata::repository::{RepositorySubscriber, SubscriberId};
use repo_metadata::{Repository, RepositoryUpdate, RepositoryWatchMode};
use warpui::r#async::SpawnedFutureHandle;
use warpui::{Entity, ModelContext, ModelHandle};

use super::{GitRepoStatusEvent, GitStatusMetadata};
use crate::code_review::diff_state::DiffStats;
use crate::context_chips::display_chip::GitBranchTrackingStatus;
use crate::context_chips::git_branch_on_click::WORKTREE_LIST_SEPARATOR;
use crate::throttle::throttle;

/// Per-repository model that owns the filesystem watcher and exposes git status
/// metadata. Consumers hold a `ModelHandle<GitRepoStatusModel>` and subscribe
/// to its events directly — no path-filtering required.
///
/// When all strong handles are dropped the model (and its watcher) is
/// automatically torn down.
pub struct LocalGitRepoStatusModel {
    repo_path: PathBuf,
    repository: ModelHandle<Repository>,
    subscriber_id: Option<SubscriberId>,
    metadata: Option<GitStatusMetadata>,
    computing_metadata_handle: Option<SpawnedFutureHandle>,
    metadata_refresh_pending: bool,
}

impl Entity for LocalGitRepoStatusModel {
    type Event = GitRepoStatusEvent;
}

impl LocalGitRepoStatusModel {
    /// Create a new per-repo status model, set up the filesystem watcher, and
    /// kick off the initial metadata computation.
    pub(super) fn new(
        repo_path: PathBuf,
        repository_model: ModelHandle<Repository>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let mut model = Self {
            repo_path: repo_path.clone(),
            repository: repository_model.clone(),
            subscriber_id: None,
            metadata: None,
            computing_metadata_handle: None,
            metadata_refresh_pending: false,
        };

        // Kick off initial metadata computation.
        model.refresh_metadata(ctx);

        // Start watching for filesystem changes.
        let (repository_update_tx, repository_update_rx) = async_channel::unbounded();
        let (throttled_tx, throttled_rx) = async_channel::unbounded();
        let start = repository_model.update(ctx, |repo, ctx| {
            repo.start_watching(
                RepositoryWatchMode::GitRepository,
                Box::new(GitStatusRepositorySubscriber {
                    repository_update_tx,
                }),
                ctx,
            )
        });
        model.subscriber_id = Some(start.subscriber_id);

        // Handle watcher registration.
        ctx.spawn(start.registration_future, |me, result, ctx| {
            if let Err(err) = result {
                log::warn!("GitRepoStatusModel: watcher registration failed: {err}");
                if let Some(subscriber_id) = me.subscriber_id.take() {
                    me.repository.update(ctx, |repo, ctx| {
                        repo.stop_watching(subscriber_id, ctx);
                    });
                }
            }
        });

        // Stream raw updates; determine whether a throttled metadata refresh is warranted.
        {
            let throttled_tx_clone = throttled_tx;
            ctx.spawn_stream_local(
                repository_update_rx,
                move |_me, update: RepositoryUpdate, _ctx| {
                    if Self::should_refresh_metadata(&update) {
                        let _ = throttled_tx_clone.try_send(());
                    }
                },
                |_, _| {},
            );
        }

        // Throttled metadata refresh (at most once every 5 seconds).
        ctx.spawn_stream_local(
            throttle(Duration::from_secs(5), throttled_rx),
            |me, _, ctx| {
                me.refresh_metadata(ctx);
            },
            |_, _| {},
        );

        model
    }

    /// Read the current metadata.  Returns `None` if metadata hasn't been
    /// computed yet.
    pub fn metadata(&self) -> Option<&GitStatusMetadata> {
        self.metadata.as_ref()
    }

    /// The path to the repository root.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Request a metadata refresh, coalescing requests while one is already running.
    pub fn refresh_metadata(&mut self, ctx: &mut ModelContext<Self>) {
        if self.computing_metadata_handle.is_some() {
            self.metadata_refresh_pending = true;
            return;
        }

        let repo_path_buf = self.repo_path.clone();
        self.computing_metadata_handle = Some(ctx.spawn(
            async move { Self::load_metadata(repo_path_buf).await },
            |me, result, ctx| {
                me.computing_metadata_handle.take();
                me.handle_metadata_result(result, ctx);
                if std::mem::take(&mut me.metadata_refresh_pending) {
                    me.refresh_metadata(ctx);
                }
            },
        ));
    }

    // ── internal helpers ─────────────────────────────────────────────

    fn handle_metadata_result(
        &mut self,
        result: anyhow::Result<GitStatusMetadata>,
        ctx: &mut ModelContext<Self>,
    ) {
        match result {
            Ok(metadata) => {
                self.metadata = Some(metadata);
            }
            Err(e) => {
                log::warn!("GitRepoStatusModel: metadata load failed: {e}");
                self.metadata = None;
            }
        }
        ctx.emit(GitRepoStatusEvent::MetadataChanged);
    }

    /// Decide whether a `RepositoryUpdate` warrants a metadata refresh.
    fn should_refresh_metadata(update: &RepositoryUpdate) -> bool {
        if update.is_empty() {
            return false;
        }
        if update.commit_updated || update.index_lock_detected || update.remote_ref_updated {
            return true;
        }
        // Check if any non-ignored file was touched.
        let changed_count = update
            .added
            .iter()
            .chain(&update.modified)
            .chain(&update.deleted)
            .chain(update.moved.keys())
            .chain(update.moved.values())
            .filter(|f| !f.is_ignored)
            .count();
        changed_count > 0
    }

    fn diff_stats(repository: &GitRepository) -> anyhow::Result<DiffStats> {
        let head_tree = match repository.head().and_then(|head| head.peel_to_tree()) {
            Ok(tree) => Some(tree),
            Err(error) if matches!(error.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => {
                None
            }
            Err(error) => return Err(error.into()),
        };
        let mut options = DiffOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        let diff =
            repository.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut options))?;
        let stats = diff.stats()?;
        Ok(DiffStats {
            files_changed: stats.files_changed(),
            total_additions: stats.insertions(),
            total_deletions: stats.deletions(),
        })
    }

    fn branch_tracking_status(
        repository: &GitRepository,
        current_branch_name: &str,
    ) -> GitBranchTrackingStatus {
        let Ok(branch) = repository.find_branch(current_branch_name, BranchType::Local) else {
            return GitBranchTrackingStatus::new(current_branch_name.to_owned(), None, 0, 0);
        };
        let Ok(upstream) = branch.upstream() else {
            return GitBranchTrackingStatus::new(current_branch_name.to_owned(), None, 0, 0);
        };
        let upstream_name = upstream.name().ok().flatten().map(str::to_owned);
        let counts = branch
            .get()
            .target()
            .zip(upstream.get().target())
            .and_then(|(local, remote)| repository.graph_ahead_behind(local, remote).ok());

        match counts {
            Some((ahead, behind)) => GitBranchTrackingStatus::new(
                current_branch_name.to_owned(),
                upstream_name,
                ahead as u32,
                behind as u32,
            ),
            None => GitBranchTrackingStatus::without_counts(
                current_branch_name.to_owned(),
                upstream_name,
            ),
        }
    }

    /// Map each local branch that is checked out in a *linked* worktree to that
    /// worktree's path, mirroring what `git worktree list --porcelain` reports.
    ///
    /// Paths use forward slashes, matching git's own porcelain output on
    /// Windows, because `parse_git_worktree_paths` compares them verbatim.
    fn linked_worktree_branches(repository: &GitRepository) -> HashMap<String, String> {
        let mut branch_to_path = HashMap::new();
        let Ok(worktree_names) = repository.worktrees() else {
            return branch_to_path;
        };

        for name in worktree_names.iter().flatten() {
            let Ok(worktree) = repository.find_worktree(name) else {
                continue;
            };
            let path = worktree.path().to_string_lossy().replace('\\', "/");
            let Ok(worktree_repository) = GitRepository::open_from_worktree(&worktree) else {
                continue;
            };
            let Ok(head) = worktree_repository.head() else {
                continue;
            };
            if !head.is_branch() {
                continue;
            }
            if let Some(branch_name) = head.shorthand() {
                branch_to_path.insert(branch_name.to_owned(), path);
            }
        }

        branch_to_path
    }

    /// Build the git branch chip's dropdown lines directly from libgit2.
    ///
    /// This replaces spawning `git branch` + `git worktree list` through the
    /// session's shell, which on Windows meant a PowerShell process per
    /// refresh. The emitted shape is deliberately identical to that command's
    /// stdout so `filter_git_branch_on_click_values` keeps working untouched:
    /// `* ` marks HEAD, `+ ` marks a branch checked out in a linked worktree,
    /// branches are ordered by committer date descending, and the worktree
    /// porcelain follows a record-separator line.
    fn branch_dropdown_lines(repository: &GitRepository) -> anyhow::Result<Vec<String>> {
        let linked_worktree_branches = Self::linked_worktree_branches(repository);

        // (committer time, name, is_head) so we can reproduce `--sort=-committerdate`.
        let mut branches: Vec<(i64, String, bool)> = Vec::new();
        for entry in repository.branches(Some(BranchType::Local))? {
            let Ok((branch, _)) = entry else {
                continue;
            };
            let Ok(Some(name)) = branch.name() else {
                continue;
            };
            let name = name.to_owned();
            let committed_at = branch
                .get()
                .peel_to_commit()
                .map(|commit| commit.time().seconds())
                .unwrap_or(0);
            branches.push((committed_at, name, branch.is_head()));
        }
        branches.sort_by(|left, right| right.0.cmp(&left.0));

        let mut lines: Vec<String> = branches
            .iter()
            .map(|(_, name, is_head)| {
                if *is_head {
                    format!("* {name}")
                } else if linked_worktree_branches.contains_key(name) {
                    format!("+ {name}")
                } else {
                    format!("  {name}")
                }
            })
            .collect();

        lines.push(WORKTREE_LIST_SEPARATOR.to_owned());

        for (branch_name, path) in &linked_worktree_branches {
            lines.push(format!("worktree {path}"));
            lines.push(format!("branch refs/heads/{branch_name}"));
            lines.push(String::new());
        }

        Ok(lines)
    }

    async fn load_metadata(repo_path: PathBuf) -> anyhow::Result<GitStatusMetadata> {
        blocking::unblock(move || {
            let repository = GitRepository::open(&repo_path).with_context(|| {
                format!("Unable to open Git repository {}", repo_path.display())
            })?;
            let current_branch_name = crate::util::git_native::current_branch_display(&repository)?;
            let main_branch_name =
                crate::util::git_native::main_branch_name(&repository, &current_branch_name);
            let stats_against_head = Self::diff_stats(&repository)?;
            let branch_tracking_status =
                Self::branch_tracking_status(&repository, &current_branch_name);
            let branch_dropdown_lines =
                Self::branch_dropdown_lines(&repository).unwrap_or_else(|error| {
                    log::warn!("GitRepoStatusModel: branch dropdown enumeration failed: {error}");
                    Vec::new()
                });

            Ok(GitStatusMetadata {
                current_branch_name,
                main_branch_name,
                stats_against_head,
                branch_tracking_status,
                branch_dropdown_lines,
            })
        })
        .await
    }
}

#[cfg(test)]
impl LocalGitRepoStatusModel {
    pub(crate) fn new_for_test(
        repository: ModelHandle<Repository>,
        metadata: Option<GitStatusMetadata>,
    ) -> Self {
        Self {
            repo_path: PathBuf::from("/test"),
            repository,
            subscriber_id: None,
            metadata,
            computing_metadata_handle: None,
            metadata_refresh_pending: false,
        }
    }

    pub(crate) fn set_metadata_for_test(
        &mut self,
        metadata: Option<GitStatusMetadata>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.metadata = metadata;
        ctx.emit(GitRepoStatusEvent::MetadataChanged);
    }
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;

impl Drop for LocalGitRepoStatusModel {
    fn drop(&mut self) {
        // Note: we cannot call `repository.update()` here because `Drop` does
        // not have access to `ModelContext`.  The `Repository` model will clean
        // up the subscriber when it notices the channel has been dropped.
        if let Some(handle) = self.computing_metadata_handle.take() {
            handle.abort();
        }
    }
}

// ── Repository subscriber adapter ───────────────────────────────────────────

struct GitStatusRepositorySubscriber {
    repository_update_tx: Sender<RepositoryUpdate>,
}

impl RepositorySubscriber for GitStatusRepositorySubscriber {
    fn on_scan(
        &mut self,
        _repository: &Repository,
        _ctx: &mut ModelContext<Repository>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        Box::pin(async {})
    }

    fn on_files_updated(
        &mut self,
        repository: &Repository,
        update: &RepositoryUpdate,
        _ctx: &mut ModelContext<Repository>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        let tx = self.repository_update_tx.clone();
        let update = update.clone();
        let index_lock_path = repository.git_dir().join("index.lock");
        Box::pin(async move {
            // Suppress commit_updated events while the git index is locked to
            // avoid reacting to stale intermediate state during git operations.
            if update.commit_updated && async_fs::metadata(&index_lock_path).await.is_ok() {
                return;
            }
            let _ = tx.send(update).await;
        })
    }
}
