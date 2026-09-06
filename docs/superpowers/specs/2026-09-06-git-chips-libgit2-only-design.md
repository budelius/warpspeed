# Git context chips stop running shell commands

**Status:** design
**Date:** 2026-09-06
**Scope:** the three git prompt chips take their values from the repository model alone. Their shell
generators run only for a session no repository model backs.

## Problem

Three prompt chips describe the current repository: `ShellGitBranch`, `GitBranchStatus`, and
`GitDiffStats`. Each one has two sources, and the second overwrites the first.

The first source is a shell command. `ContextChipKind::to_chip` builds all three from
`shell_builtin_with_runtime_policy` (`app/src/context_chips/mod.rs:317-340`), giving them
`builtins::shell_git_branch()`, `builtins::shell_git_branch_status()`, and
`builtins::shell_git_line_changes()`. `fetch_chip_value_once`
(`app/src/context_chips/current_prompt.rs:680`) runs whichever applies through the session's shell.

The second source is libgit2. `GitRepoStatusModel` holds an open repository handle, and
`apply_git_repo_metadata` (`current_prompt.rs:1618`) copies its metadata into the same three chips:
`current_branch_name` into `ShellGitBranch`, `branch_tracking_status` into `GitBranchStatus`, and
`stats_against_head` into `GitDiffStats`. Whatever the shell produced, this replaces.

The shell work is therefore redundant wherever a repository model exists, and it is expensive. On
Windows each run spawns a PowerShell process, measured at ~435 ms against ~16 ms for the same work in
Git Bash (commit `26e9967b`).

When the shell cannot run at all, the redundancy turns into a storm. Closing a pane leaves the chips
pointed at a dead session, `prepare_shell_command_context` returns `None`, and every refresh logs a
warning (`current_prompt.rs:753`). Three consecutive logs from 2026-09-04 hold 3,108, 4,905, and
12,591 of those warnings, split in equal thirds between `ShellGitBranch`, `GitDiffStats`, and
`KubernetesContext`, arriving in bursts of hundreds inside a single second after
`PaneHeaderAction::Close`. Each one also sets the chip to `ChipUpdateStatus::Error` and clears its
value, so a chip the repository model had already filled blanks out.

A half-built suppression already exists. `is_updated_externally` (`current_prompt.rs:1700`) returns
`true` for exactly these three chips when `git_repo_status.is_some()`, but only the periodic timer
consults it. Every other path into `fetch_chip_value_once` ignores it.

### The dropdown runs a second shell command

Commit `26e9967b` moved the branch-switcher dropdown to libgit2 by feeding
`metadata.branch_dropdown_lines` to `apply_git_branch_dropdown`, which falls back to the shell
generator only when those lines are empty. That closed one path and left another open.

`fetch_chip_value_once` takes an `on_click_generator` parameter and ends by calling
`refresh_on_click_values` on it unconditionally (`current_prompt.rs:865-867`). `ShellGitBranch` and
`GitBranchStatus` both pass `Some(builtins::shell_other_git_branches())`, so every value fetch still
spawns the branch-list command regardless of what libgit2 supplied. The dropdown data has one native
source and two shell sources.

### Remote sessions are already covered

`git_status_proto.rs` carries `current_branch_name`, `stats_against_head`, and the four
`branch_tracking_status` fields over the wire (`app/src/remote_server/git_status_proto.rs:39-45`,
`59-74`). Only `branch_dropdown_lines` arrives empty (`:64`), which is what triggers the existing
dropdown fallback. All three chip *values* therefore come from the repository model on local and
remote sessions alike.

## Goals

- A session backed by `GitRepoStatusModel`, local or remote, never runs a shell command for
  `ShellGitBranch`, `GitBranchStatus`, or `GitDiffStats`.
- The three chips display the same values they display today.
- The branch dropdown keeps working, including its shell fallback for remote sessions.
- Closing a pane produces no `could not prepare execution context` warnings for these chips, and
  leaves their values intact.

## Non-goals

- `KubernetesContext`. It shares the storm and a third of the warnings, but no native source exists
  for it. Fixing it means throttling the refresh, which is a separate problem.
- Deleting the shell generators. They remain the only source for a session with no repository model,
  and the fallback path must keep working.
- `app/src/util/git.rs`, the forge layer, and the `GitOperationsInCodeReview` flag. Those are specs
  two through four of this program.

## Design

### 1. Consult the existing predicate at the generator

`is_updated_externally` already names the condition precisely: this chip's value arrives from
somewhere else, so running a generator for it wastes work. Promote it from a timer check to a
generator check.

`fetch_chip_value_once` gains an early return, placed after the availability block and before the
`match generator`:

```rust
if self.is_updated_externally(chip_kind) {
    self.set_chip_update_status(chip_kind, ChipUpdateStatus::Cached);
    return;
}
```

Position carries most of the design.

Availability is computed and applied first, so a chip the user disabled stays disabled and still
aborts its in-flight handles. Returning above the `match` covers `PromptGenerator::ShellCommand` and
`PromptGenerator::Contextual` alike, which is right: the repository model supersedes both.

Returning above the `match` also closes the dropdown's second shell path, because
`refresh_on_click_values` runs *after* the `match` (`current_prompt.rs:865-867`). One early return
therefore suppresses the value command and the branch-list command together. No separate guard on
the `refresh_on_click_values` call is needed, and adding one would be unreachable — any chip that
reaches the end of the function has already failed the same predicate.

The status is `Cached`, not `Error` and not `Disabled`. The value is present and current; only the
generator was skipped. `Error` is what produces today's blanking, and `Disabled` would misreport a
chip the user enabled.

### 2. Remote sessions keep their dropdown

`apply_git_branch_dropdown` still receives empty `branch_dropdown_lines` from a remote session and
still calls `refresh_git_branch_dropdown`, which reaches `refresh_on_click_values` directly rather
than through `fetch_chip_value_once`. The early return never sees that path, so the remote fallback
survives unchanged.

### 3. Nothing else changes

`apply_git_repo_metadata` keeps writing all three values. `ContextChipKind::to_chip` keeps returning
the shell generators, because a session with no repository model still needs them. The emitted chip
values, their formatting, and `git_branch_on_click` parsing stay as they are.

## Testing

Extend `app/src/context_chips/current_prompt_tests.rs`:

- With a `GitRepoStatusModel` attached, a refresh of each of the three chips runs no shell command
  and leaves the value the repository model wrote. Assert on the recorded command count, not on
  timing.
- With no repository model attached, each chip still runs its shell generator. This is the guard
  against regressing sessions that depend on the shell path.
- A chip outside the three, such as `KubernetesContext`, still runs its generator with a repository
  model attached. This pins the predicate's scope.
- With a repository model whose `branch_dropdown_lines` are empty, standing in for a remote session,
  the dropdown still populates through the shell fallback.
- After the session backing a chip goes away, a refresh logs no warning and clears no value.

## Risks

A session whose repository model exists but stops updating would now show a stale value where it
previously showed a shell-refreshed one. The exposure is small: `GitRepoStatusModel` drives the
values from a filesystem watcher, and the same staleness already applies to every value
`apply_git_repo_metadata` writes, because it wins the race against the shell result today.

## Follow-on specs

1. **This spec.** The git chips stop shelling out.
2. `app/src/util/git.rs` read operations move to libgit2. Seven functions, no unknowns.
3. The forge layer: a `ForgeClient` trait over GitHub, GitLab, and Gitea, a layered credential
   resolver, and host probing with a cache.
4. Enable `GitOperationsInCodeReview`, once the forge layer gives it something native to call.
