# WarpSpeed Stabilization and Feature Completion

Date: 2026-09-13
Status: Proposed implementation sequence; inventory verified against fetched Git refs and GitLab.
Scope: WarpSpeed first, with local workspace migration tracked as a separate operational lane.

## Outcome

Provide a reproducibly installed macOS app with dependable session restoration, attach/resume, and
busy-session behavior. Reconcile existing feature work before adding new session infrastructure.
Finish multi-forge support in independent follow-up slices.

## User stories and acceptance criteria

### US1 — Reliable local installation (P0)

The user can build, install, and launch the OSS application beside other Warp installations.

- Native macOS build and signature verification pass without a private channel-config generator.
- Installation does not depend on Finder automation successfully formatting a DMG.
- The installed app starts its supervisor and opens a usable terminal.
- Supervisor socket paths fit platform limits while preserving ownership/security checks; cover
  long home paths and profiles, not just the current machine's pathname.
- Display name, bundle identity, CLI entry points, and data directory are documented. Renaming the
  bundle to `warpseed.app` alone is not a complete in-code branding change.
- The current uncommitted OSS persistent-session feature-flag removal is reviewed independently;
  do not silently restore it or assume it belongs to the Mac build fix.

### US2 — Sessions are recoverable and safe to reopen (P1)

- Detaching the last visible pane preserves a window and a way back.
- Running and recent Claude Code, Codex, and Pi sessions are discoverable in the palette.
- A missing supervisor or deleted project directory produces usable recovery behavior, not a panic.
- Busy-session handling offers only valid choices. Cloning gets an independent identity and cannot
  steal the original lease, ambient agent pane, or checkpoint state.
- Palette ordering, loading, failure feedback, and cold-start recent-session rows are verified.
- Debug builds do not assert when a pane is handed off before its PTY receives its initial size.

### US3 — Code review and forge operations are dependable (P2)

- The blank diff-area report is reproduced and diagnosed from recorded counts/viewport evidence;
  add a regression for the proven cause before changing behavior.
- GitLab.com and self-hosted GitLab merge-request lookup work using host-scoped credentials.
- Gitea support and PR/MR creation are separate reviewable slices.
- Keep existing CLI fallback until native clients achieve feature parity and verification passes.

### US4 — Local project migration is auditable (operational)

- Imported config files parse; tab references resolve; shell executables exist.
- Existing projects and branches remain unchanged. Missing repos are cloned to mapped destinations.
- Missing/ambiguous repositories are reported, never substituted speculatively.
- TM clone retries preserve any existing or partially populated destination.

## Non-goals

- Reimplementing features already contained in `main` because old checkboxes remain unchecked.
- Bulk merging the Windows handoff snapshot or retiring branch names without review.
- Shipping remote persistent-session transports as part of this stabilization slice.
- Copying live Windows PTYs to macOS or committing user session transcripts/credentials.

## Evidence and remaining uncertainty

The feature inventory is in `plan.md`. No new Rust test, Clippy, GUI, or cloud verification run is
claimed by these documents. Prior build/launch checks apply to the earlier installed artifact, not
necessarily every current uncommitted edit or feature branch.
