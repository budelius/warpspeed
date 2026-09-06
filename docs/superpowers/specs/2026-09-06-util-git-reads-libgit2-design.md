# `util/git.rs` read operations move to libgit2

**Status:** design
**Date:** 2026-09-06
**Scope:** the read-only helpers in `app/src/util/git.rs` answer from an in-process libgit2
repository instead of spawning `git`.

## Problem

`app/src/util/git.rs` asks git questions by running git. Every helper routes through
`warp_util::git::run_git_command`, which spawns a process per call. The file holds 31 such call
sites.

Meanwhile the same questions already have libgit2 answers elsewhere in the tree.
`GitRepoStatusModel` opens a repository once and computes the current branch
(`app/src/code_review/git_repo_model/local.rs:181`), the main branch (`:201`), and diff stats
(`:217`) from that handle. Those helpers are private to the model, so `util/git.rs` reimplements the
same lookups as subprocesses.

The cost is per call and it is worst where it is most frequent. Spawning a process on Windows is
expensive enough that the same work measured ~435 ms through PowerShell against ~16 ms natively
(commit `26e9967b`).

### The shell surface is larger than previously stated

An earlier reading of this program's scope claimed `app/src/code_review/` executes no commands. That
was wrong. It executes them through the imported `run_git_command` rather than by constructing a
`Command`, so a search for `Command::new` missed all of it. The real surface:

| File | `run_git_command` call sites |
| --- | --- |
| `app/src/util/git.rs` | 31 |
| `app/src/code_review/diff_state/local.rs` | 29 |
| `crates/repo_metadata/src/repository.rs` | 1 |

The subcommands invoked across those sites are `rev-parse` (13), `diff` (12), `symbolic-ref` (3),
`status` (3), `show` (3), `ls-files` (3), `log` (3), `rm` (2), `merge-base` (2), `branch` (2), and
one each of `restore`, `reset`, `push`, `fetch`, `commit`, `checkout`, and `add`.

This spec takes the first row only, and only its read-only half. The reasoning is in Non-goals.

## Goals

- These helpers open a libgit2 repository and answer from it, spawning no process:
  `list_local_branches_sync`, `detect_current_branch`, `detect_current_branch_display`,
  `detect_main_branch`, `get_committed_branch_file_entries`, `compute_unpushed_state`,
  `get_diff_for_pr`, and `get_branch_commit_messages`.
- Each returns what its shell implementation returned, including the documented edge cases: the
  literal `"HEAD"` when detached, a short SHA from the `_display` variant, `0` additions and
  deletions for a binary file, and an empty vector when no merge base exists.
- No libgit2 work runs on the foreground executor.
- The libgit2 branch helpers have one implementation, not two.

## Non-goals

- `app/src/code_review/diff_state/local.rs`. Its 29 sites include `rm -f`, `reset`, `restore`,
  `checkout --`, and `clean`, which delete and overwrite uncommitted work. Reimplementing a
  destructive path on a different git implementation risks losing user data, and "spawns no process"
  is a weak reason to accept that risk. It needs its own spec, its own tests, and a deliberate
  decision that the trade is worth taking.
- `run_push`, `fetch`, and anything else needing the network. `git2` is declared with
  `default-features = false` and only `vendored-libgit2` (`app/Cargo.toml:321`), so the crate has no
  HTTPS or SSH transport compiled in. Network operations need that feature *and* the credential
  chain, which is spec four.
- `gh`. That is spec three, the forge layer.
- `crates/repo_metadata/src/repository.rs`. One call site in another crate; it follows whichever
  pattern this spec establishes.

## Design

### 1. One home for the libgit2 helpers

`current_branch_name`, `main_branch_name`, and `linked_worktree_branches` are general repository
questions that happen to live on `LocalGitRepoStatusModel`. Move them to a new
`app/src/util/git/native.rs` as free functions taking `&git2::Repository`.
`git_repo_model/local.rs` calls them from their new home; its behavior does not change, and its
existing tests keep passing unmodified. That last point is the check on whether the move was clean.

The new module also holds the lookups this spec adds: merge base, revwalk, numstat, and patch text.

### 2. Every helper offloads with `blocking::unblock`

git2 is synchronous, and these are `async fn` called from view code. The established pattern is
already in the tree — `load_metadata` wraps its whole repository session in `blocking::unblock`
(`git_repo_model/local.rs:360`) — and each converted helper adopts it:

```rust
pub async fn detect_current_branch(repo_path: &Path) -> Result<String> {
    let repo_path = repo_path.to_owned();
    blocking::unblock(move || {
        let repository = git2::Repository::open(&repo_path)?;
        native::current_branch_name(&repository)
    })
    .await
}
```

One `unblock` per public helper, not one per lookup, so a helper answering several questions opens
the repository once and holds it for all of them. `get_committed_branch_file_entries` needs the main
branch, a merge base, and a diff; all three run inside a single closure against a single handle.

`list_local_branches_sync` stays synchronous, because its callers are synchronous view handlers. It
keeps its "empty set on any failure" contract, and it gets cheaper rather than merely different: it
currently blocks the caller on a process spawn.

### 3. Mapping each helper

| Helper | libgit2 |
| --- | --- |
| `list_local_branches_sync` | `repo.branches(Some(BranchType::Local))` |
| `detect_current_branch` | `repo.head()`; unborn or detached yields `"HEAD"` |
| `detect_current_branch_display` | as above, then `peel_to_commit` and a short SHA when detached |
| `detect_main_branch` | `refs/remotes/origin/HEAD` symbolic target, then the existing fallbacks |
| `get_committed_branch_file_entries` | `merge_base`, then `diff_tree_to_tree` and per-delta line counts |
| `compute_unpushed_state` | `branch.upstream()`, then a revwalk of `upstream..HEAD` |
| `get_diff_for_pr` | `diff_tree_to_tree` rendered with `Diff::print`, truncated as today |
| `get_branch_commit_messages` | revwalk of `base..HEAD`, taking `commit.summary()` |

`get_diff_for_pr` keeps its existing `MAX_DIFF_CHARS_FOR_AI` truncation and its
`origin/<branch>`-then-`HEAD` end-ref preference, unchanged.

### 4. Numstat parity is the subtle one

`get_committed_branch_file_entries` parses `git diff --numstat`, where a binary file renders as
`-\t-\t<path>` and the current code turns the unparsable counts into `0`. The libgit2 equivalent
must produce the same shape: a binary delta reports zero additions and zero deletions rather than
being dropped. Renames are the other divergence to pin, because numstat's output depends on rename
detection being on; the diff options set `find_renames` to match.

## Testing

The shell implementation is the oracle. Before deleting it, each converted helper is tested against
it.

`app/src/util/git_tests.rs` gains a fixture builder that constructs real repositories with git2 —
commits, branches, a detached HEAD, a binary file, a rename, a repository with no commits at all, and
a branch with no upstream. For each fixture and each helper, the test asserts the libgit2 result
equals the shell result. Those parity tests are gated on `git` being on `PATH` and skipped when it is
not, so the suite still runs on a machine without git.

Alongside them, ordinary unit tests assert the documented edge cases directly, without needing the
oracle: detached HEAD returns `"HEAD"`, the `_display` variant returns a short SHA, a binary file
yields `0`/`0`, and a missing merge base yields an empty vector. These are the tests that survive
after the shell implementation is gone.

The existing `git_repo_model/local_tests.rs` must pass unmodified, proving the helper move in
section 1 changed no behavior.

## Risks

- **Silent behavior drift.** git2 and the git CLI disagree at the edges — `core.autocrlf`,
  `.gitattributes`, rename detection thresholds. The parity tests exist to find the disagreements
  that matter; the fixtures deliberately include the cases where the two are known to differ.
- **`detect_main_branch` heuristics.** It reads `refs/remotes/origin/HEAD`, which is absent in many
  clones, and then falls through several guesses. The fallback order is preserved exactly rather
  than tidied, because a repository that resolves differently changes which commits every downstream
  helper reports.
- **Vendored libgit2 without transport features.** Correct for reads and stated here so that the
  first network operation added later does not fail confusingly. Adding `https` to the feature list
  is spec four's job, not this one's.
