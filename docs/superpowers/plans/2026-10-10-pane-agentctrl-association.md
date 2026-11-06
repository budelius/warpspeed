# Durable pane AgentCtrl association plan

**Goal:** Remember the most recently attached AgentCtrl run for each terminal pane across app restarts and select that run when the pane opens Sessions again.

**Architecture:** Store an optional validated AgentCtrl job ID in each terminal pane snapshot and its existing SQLite row. Copy it into the runtime `TerminalPane` on restore, update it when the Sessions browser successfully queues an attach command, and use it as the browser's preferred selection after Fleet data arrives.

## Tasks

1. Add a nullable `managed_session_job_id` terminal-pane persistence column, migration, snapshot field, and persistence round-trip tests.
2. Preserve the association in `TerminalPane` snapshots and expose narrow `PaneGroup` get/set methods.
3. Record the job only after the viewer command is queued successfully.
4. Prefer the associated AgentCtrl row when Sessions opens, retaining the preference until the asynchronous Fleet refresh supplies that row.
5. Add snapshot, workspace-routing, and selection tests; run formatting, focused tests, strict targeted Clippy, and `git diff --check`.
6. Push the feature branch to Gitea and GitLab, merge with `--no-ff` into `dev`, and verify both remote refs without installing or restarting WarpSpeed.
