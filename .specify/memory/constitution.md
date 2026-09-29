# WarpSpeed Constitution

Version: 1.0.0
Ratified: 2026-09-13

## Authority

`AGENTS.md` is the source of truth for architecture, coding conventions, testing, and submission
requirements. This constitution adds planning rules; it does not replace those instructions.

## Principles

1. Preserve user data: never overwrite existing checkouts, session identities, or credentials during
   migration. Back up configuration changes and keep rollback possible.
2. Distinguish implemented, integrated, and verified work. A commit or unchecked historical plan is
   not evidence of either successful verification or missing implementation.
3. Keep independent changes on their respective feature branches/worktrees. Do not fold unrelated
   working-tree edits or Windows handoff changes into a release without review.
4. Validate session continuity through real lifecycle transitions, not compilation alone. Keep
   terminal model locking and UI-thread work bounded according to `AGENTS.md`.
5. Verify locally first, then use the smallest relevant platform matrix. Record failures and gaps;
   never present unrun checks as passing. Follow repository formatting and Clippy gates before a PR.
6. Keep credentials host-scoped and out of logs, specifications, and committed migration artifacts.

## Planning and Changes

Each implementation slice needs acceptance criteria, a target branch, regression coverage, and an
explicit integration dependency. Existing design documents remain authoritative for their feature;
new release plans link to them rather than duplicating their designs. Change this constitution only
when a concrete project requirement changes, documenting the reason and version increment.
