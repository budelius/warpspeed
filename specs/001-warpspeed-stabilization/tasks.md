# Tasks: WarpSpeed Stabilization

These are remaining actions, not a transcription of stale historical checkboxes. Feature code
already in branches is recorded in `plan.md`; none of its tests are marked passed here.

## Planning setup

- [x] T001 Fetch refs and inventory open GitLab MR/issues and branch ancestry.
- [x] T002 Upgrade Specify CLI to official release 1.0.6 and verify its reported version.
- [x] T003 Initialize generic Spec Kit assets without replacing agent-specific configuration.
- [x] T004 Write constitution, acceptance criteria, inventory, phased plan, and remaining tasks.

## Phase 0 — Mac baseline and debug lifecycle (P0)

- [x] T005 Record/backup dirty-tree changes; separate the OSS flag delta from the two Mac fixes.
- [x] T006 Add Mac socket-path regression tests, including long home paths and profiles; retain
      protected directory ownership checks and avoid same-user profile/channel collisions.
- [x] T007 Complete a robust socket-path solution and run focused tests.
- [x] T008 Add a repeatable side-by-side Mac install path that does not require DMG Finder styling.
- [ ] T009 Build/sign/install an exact commit and verify a usable shell and supervisor lifecycle.
- [ ] T010 Resolve the intended OSS persistent-session flag and branding behavior with the user.
- [ ] T011 In `fix/grid-handoff-before-pty-size`, reproduce the MR-reported grid assertion.
- [ ] T012 Fix initial PTY-size/handoff ordering and test debug detach/close/reopen transitions.

## Phase 1 — Session features (P1)

- [ ] T013 Validate session-in-use cloning already on dev: busy prompt, lease owner, clone identity,
      rollback on failure, agent-pane isolation, palette dedup, and feature gating.
- [ ] T014 Reconcile MR !1 with dev in its own worktree; review overlapping palette/session changes.
- [ ] T015 Run scanner, ordering, resume-tab, last-visible-pane, and attach/detach regressions.
- [ ] T016 Visually verify palette and get-started rows with supervisor absent and cwd deleted.
- [ ] T017 Add mixer-ordering regression; decide relevance truncation behavior beyond 20 matches.
- [ ] T018 Implement loading feedback or remove unused history state; verify stale-attach errors
      without adding synchronous work to the keystroke path.
- [ ] T019 Reproduce/isolate order-dependent pane-group failures and document environment blockers.
- [ ] T020 Run format, prescribed Clippy, and applicable Mac/Windows/Linux verification; then request
      review/promotion approval. Do not push with failing mandatory checks.

## Phase 2 — Diagnostics and Windows handoff (P1/P2)

- [ ] T021 [P] Reproduce blank code-review diff area and capture diagnostic counts/viewport evidence.
- [ ] T022 Add a failing regression for the proven cause, fix it, and visually verify the diff area.
- [ ] T023 [P] Classify each Windows handoff hunk; identify superseded, duplicate, and unique changes.
- [ ] T024 Move unique changes to their corresponding feature branches and verify Windows snap,
      resizing/repaint, persistence, and handoff behavior without bulk-merging the WIP snapshot.

## Phase 3 — Forge completion (P2)

- [ ] T025 Specify GitLab client acceptance cases and create `feat/forge-gitlab` when baseline is ready.
- [ ] T026 Implement GitLab MR lookup, nested namespaces, host-scoped auth, and typed failures.
- [ ] T027 Test with fake credentials/HTTP fixtures, then explicitly scoped public/self-hosted checks.
- [ ] T028 Specify and implement Gitea support as an independent slice.
- [ ] T029 Implement native PR/MR creation as a separate slice; retire CLI fallback only after parity.

## Phase 4 — Remote sessions (P3)

- [ ] T030 Audit existing local supervisor protocol/ring/transport code; do not reimplement it.
- [ ] T031 Write a remote-session feature spec for SSH and agent-ctrl with reconnect, backpressure,
      authorization, and session ownership acceptance criteria.
- [ ] T032 Split terminal and agent-ctrl implementation into their respective repositories/branches.

## Operational lane

- [ ] T033 Retry TM clone DNS failures and verify repository HEADs without touching existing work.
- [ ] T034 Obtain confirmed CEFA and really-old-stuff repository locations; clone/map only after that.
- [ ] T035 Revalidate current config parsing, references, directories, shells, and representative
      session launches after migration and any subsequent runtime saves.

## Dependency/parallelism notes

T006–T009 and T011–T012 are independent until lifecycle verification. T013 precedes final validation
of T014–T020. T021 and T023 can proceed in isolated worktrees while session work is reviewed.
Forge and remote work should not delay the Mac/session stabilization release. Operational tasks
must not alter existing feature branches or commit user configuration into WarpSpeed.
