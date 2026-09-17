# Implementation Plan

Date: 2026-09-13
Spec: [spec.md](spec.md)
Tasks: [tasks.md](tasks.md)

## Baseline and evidence

Inventory from `git fetch origin`, commit ancestry, source inspection, and GitLab API:

- `origin/main`: `3b78dd8a`.
- `origin/dev`: `ffe4ad05`, 19 commits ahead of main, including session-in-use cloning.
- One open MR: [!1 — Attaching sessions](https://gitlab.com/martin-wieser/warpspeed/-/merge_requests/1),
  `feat/agent-session-attach` → `dev`; API reports conflicts.
- Zero open GitLab issues at inventory time. Branches and designs are therefore essential backlog
  sources, but absence of an issue does not imply a feature is finished.
- Current branch: `fix/macos-local-build`; uncommitted changes in `app/src/app_menus.rs`,
  `crates/warp_terminal/src/local_tty/server/persistent.rs`, and `app/src/bin/oss.rs`.
  The last removes the explicit persistent-launch-session flag and must be reviewed separately.
- Spec Kit upgraded from `0.7.3.dev0` to official release `1.0.6`, commit
  `96c9bd657bfd5de0d651a6165084932b7304ac99`. Generic integration is under `.specify/commands`.
  No existing agent-specific setup was overwritten.

## Open feature inventory

| Workstream | Evidence/status | Next action |
| --- | --- | --- |
| Mac build/install | Compiler borrow fix and shortened socket names are uncommitted; earlier OSS artifact built, signed, and launched | Regression tests, long-path design, repeatable install, review OSS flag delta |
| Busy-session dialog/clone/palette dedup | `feature/session-in-use-clone` implemented and merged into `dev`, not `main` | Verify lease ownership, clone rollback, independent state, feature gating; promote after gates |
| Agent attach/history | `feat/agent-session-attach`, open MR !1, conflicting with `dev` | Resolve in its own worktree; verify the combined palette/session model |
| Grid handoff ordering | MR !1 reports deterministic debug assertion at `crates/warp_terminal/src/model/grid/storage.rs:380`, explicitly not fixed there | Separate regression and ordering fix before trusting debug lifecycle tests |
| Blank code-review diff area | `diag/code-review-blank-diff-area` contains diagnostic commit `192a90ee`, not a root-cause fix | Reproduce and capture file count, row count, viewport height, out-of-range render evidence |
| Windows-to-Mac handoff | `wip/windows-to-mac-2026-09-11`, `c846d7fb`: 19-file WIP atop diagnostic commit | Classify hunks by feature; compare against dev/attach/Mac fixes; transplant only reviewed deltas |
| Multi-forge support | GitHub client and credential/detection seams exist; `app/src/forge/resolve.rs` explicitly says GitLab/Gitea clients are absent | GitLab first, Gitea next, PR/MR creation separately; retain CLI fallback |
| Remote persistent sessions | Design exists for SSH/agent-ctrl; local protocol/framing/ring/transport modules already exist | Audit implemented seams and write a new remote-specific spec before implementation |

### MR !1 known gaps (reported, not independently reproduced here)

- Palette and get-started rows have not been visually verified according to the MR description.
- Deleted cwd and missing supervisor need manual coverage.
- More than 20 matching historical sessions are truncated by recency rather than relevance.
- Mixer-level ordering regression is missing.
- `AgentSessionHistoryModel::state()` is unused; no loading row was implemented.
- Running-session results may be stale until the next palette open; attach failure feedback needs
  verification. Do not put synchronous supervisor queries back on each keystroke.
- Reported historical test failures include container/environment issues and two order-dependent
  pane-group tests. Reproduce on the target environment instead of carrying forward pass claims.

### Not new implementation work

These branch tips are ancestors of `main`: `feature/session-history-launcher`,
`fix/git-bash-repaint-duplication`, `fix/pty-resize-storm-on-window-drag`, and
`perf/native-git-branch-dropdown`. Verify regressions as needed; do not reimplement them.

Git-chip libgit2 work is merged in main; `perf/git-chips-libgit2-only` currently has the same extra
blank-diff diagnostic commit as the diagnostic branch. Its branch name does not represent an
unmerged performance feature. Local supervisor framing/ring/transport and lazy restore also have
implementation evidence; historical unchecked plans are not a reliable completion ledger.

## Execution order and branch isolation

### Phase 0 — Stabilize the baseline

Use `fix/macos-local-build` for Mac fixes only. Inventory/backup the dirty tree before any branch
operation. Establish tests for the socket-path regression and decide how unusually long homes and
profiles are handled without weakening directory ownership checks. Make direct app installation
repeatable and verify terminal startup, not just process existence. Review the OSS flag delta with
its author/user before incorporating it.

Create a separate `fix/grid-handoff-before-pty-size` worktree from the agreed integration baseline.
Reproduce the reported assertion, fix ordering, and validate detach/close/reopen in debug builds.

### Phase 1 — Converge session features

Treat `dev` as the current feature integration baseline, not an implicitly releasable build.
Verify the already-merged clone feature, then reconcile `feat/agent-session-attach` with it. Use
separate worktrees; no forced checkout over local changes. Run ownership/clone and attach/history
regressions together because they share palette, session identity, and lifecycle paths.

Complete visual verification and remaining small UX/test gaps on the attach branch. Promote to
main only after repository-required formatting, Clippy, tests, and applicable platform checks pass.
Do not push or merge automatically as part of this planning task.

### Phase 2 — Reconcile diagnostics and handoff

Work the blank-diff reproduction independently on its diagnostic branch. Split the Windows WIP
snapshot by affected feature and reconcile against the stabilized session work. Keep Windows snap,
PTY sizing/repaint, session persistence, and code-review changes reviewable separately.

### Phase 3 — Forge support

Create `feat/forge-gitlab` from the stabilized baseline. Implement API-backed MR lookup for public
and self-hosted GitLab using fake-store/stub-HTTP regression tests; include nested namespaces,
credential-host isolation, missing auth, 404, and network failure behavior. Verify against TM only
with an explicitly scoped authenticated check; do not persist credentials in project files.

Follow with separate Gitea and PR/MR-creation slices. Remove `gh` fallbacks only after parity.
Reference: `docs/superpowers/specs/2026-09-08-forge-layer-design.md`.

### Phase 4 — Remote session roadmap

Audit SSH and agent-ctrl integration against the existing design. Define reconnect offsets,
backpressure, authorization, ownership, and disconnect semantics before implementation. Track
agent-ctrl changes in its own repository and do not mix them into terminal migration work.
Reference: `docs/superpowers/specs/2026-08-28-remote-persistent-sessions-design.md`.

## Verification gates

1. Smallest focused tests and compile checks first; use repository test skills when implementing.
2. Real GUI checks for install, palette, busy-session dialogs, attach/detach, and window lifecycle.
3. Run `./script/format` and the exact Clippy configuration required by `script/presubmit` before
   any PR/branch push; all mandatory gates must pass.
4. Apply cross-platform verification skill after local gates. Mac-specific socket/install changes
   require Mac evidence; shared session changes require Mac/Windows/Linux checks as applicable.
   No cloud runs were launched for this documentation task.
5. Record tested commit and installed artifact so later rebuilds do not inherit old verification.

## Operational lane (outside the product backlog)

- TM inventory: 212 accessible repositories; batch completed with 198 cloned, 11 existing, 3 DNS
  failures (`tm/Parsy`, `tm/RulesEngine`, `tm/SSOBrowser`). Retry only failed, absent destinations.
- `HOH/hoh-website` mapped to GitLab `pseekoo/house-of-hera`; `JustVIBE/UngarnPartner` mapped to
  GitHub `Aran26-cyber/ungarn-partner.at`. Both cloned. `wandrer/quote-bot` also cloned.
- CEFA still needs a confirmed domain/owner/remote; `STOZ3N/really-old-stuff` remains unresolved.
- Revalidate imported configs against current files and active app state; previous successful
  conversion is not proof that later runtime saves have preserved every path.
- Keep detailed clone/config reports in local operational storage, not in this repository.

## Open decisions

- Confirm whether persistent launch sessions should be enabled by default in the OSS channel.
- Confirm whether `warpseed` is only the installed filename or the intended full product branding.
- Obtain CEFA and really-old-stuff remotes rather than guessing unrelated repositories.
