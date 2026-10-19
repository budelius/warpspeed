# Quickstart: Validate WarpSpeed Session Performance and Reliability

## Prerequisites

- Checkout branch `001-warpspeed-session-performance`.
- Install repository build and test dependencies.
- Keep original user checkpoints untouched; use copied fixtures or generated equivalents.
- For process tests, use an isolated `WARPSPEED_PROFILE`.

## 1. Focused unit validation

```bash
cargo nextest run -p warp -E 'test(prompt_chip) | test(session_store) | test(serialized_block) | test(single_instance)'
```

Expected:

- Legacy integer-array and V2 base64 checkpoints both deserialize.
- Per-block, pane, window, checkpoint-file, and retained-history cardinality budgets are enforced.
- Interrupted migration leaves a valid source or destination.
- Restored blocks do not launch prompt commands.
- Non-repository Git state does not launch Git.
- Diagnostic excerpts, queues, and files remain bounded.

## 2. Legacy checkpoint migration

Use a copied structurally representative V1 checkpoint with dense ANSI output.

1. Record its checksum and byte size.
2. Restore it under an isolated profile.
3. Confirm layout, pane metadata, commands, recent output, and resume descriptors.
4. Confirm the active checkpoint is V2 and no more than 25 MiB.
5. Confirm a checksummed `.v1.bak` remains.
6. Interrupt migration at each injected transaction boundary and rerun restoration.

Expected: every run retains at least one valid checkpoint, and corrupted block output does not prevent layout restoration.

### Reference Windows benchmark

Run on 2026-10-18 against a copied 142,723,686-byte STOZN V1 checkpoint with the final debug test build:

- bounded V1 parse: 5,302 ms;
- validated migration, including rollback copy: 3,006 ms;
- total first migration path: 8,312 ms;
- V2 size: 14,886,364 bytes;
- V2 reread: 1,067 ms;
- retained decoded output: 11,079,008 bytes;
- sampled peak working set: 43,401,216 bytes;
- sampled peak private memory: 25,456,640 bytes;
- child processes during parse/migration/reread: 0;
- warnings in captured benchmark output: 0.

The source checkpoint was opened read-only and copied to a temporary directory. The test retained and checksum-validated the temporary `.v1.bak`; it did not modify the user profile.

The release-mode test was repeated after integrating the latest `dev` branch:

- bounded V1 parse: 665 ms;
- validated migration, including rollback copy: 353 ms;
- total first migration path: 1,018 ms;
- V2 size: 14,886,364 bytes;
- V2 reread: 74 ms;
- retained decoded output: 11,079,008 bytes;
- sampled peak working set: 23,003,136 bytes;
- sampled peak private memory: 13,639,680 bytes;
- child processes during parse/migration/reread: 0;
- runtime warnings during parse/migration/reread: 0.

## 3. Restoration side-effect validation

Restore a generated 1,000-block session while recording prompt executor counts and logs.

Expected:

- zero prompt helper commands during replay;
- zero per-restored-block prompt or filter warnings;
- at most one prompt refresh after live context becomes ready.

Then start in a known non-repository directory.

Expected: zero Git prompt processes and no Git help text in prompt diagnostics.

## 4. Prompt diagnostic stress

Run the logger concurrency test with two writer processes and oversized stdout/stderr fixtures.

Expected:

- no NUL-filled gaps;
- each stream excerpt is at most 4 KiB;
- original lengths and truncation flags are present;
- queue pressure does not block the caller;
- default retained files total no more than 30 MiB.

## 5. Profile ownership

On each affected platform:

1. Start a GUI under `WARPSPEED_PROFILE=session-performance-test`.
2. Launch the same executable ten more times.
3. Confirm each launch opens or forwards to the primary.
4. Confirm only one GUI writer owns that profile.
5. Repeat from a different executable version.
6. Start `WARPSPEED_PROFILE=session-performance-test-2` and confirm it owns an independent primary.
7. Confirm CLI commands and terminal supervisors do not acquire GUI ownership.
8. Invoke the window-process restart action and confirm WarpSpeed requests a profile-owner restart instead of starting a replacement GUI writer.
9. Confirm whitespace-only, unsupported-character-only, and case-only-different profile names remain isolated from the default profile and from each other.

Expected: one primary per profile, no concurrent old/new writers, and live supervisor-managed terminals remain attachable after the GUI restart.

## 6. Repository quality gates

```bash
./script/format
cargo clippy --workspace --exclude warp_completer --all-targets --tests -- -D warnings
cargo clippy -p warp --all-targets --tests -- -D warnings
cargo clippy -p warp_completer --all-targets --tests -- -D warnings
cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2
cargo test --doc
git diff --check
```

## 7. Cross-platform verification

After local checks pass, discover current Oz runners and select one representative runner for each affected OS. Verify Windows named-mutex and atomic-replace behavior, Linux D-Bus and atomic-replace behavior, and macOS checkpoint round-trip compatibility. No additional architecture is required unless the final diff introduces architecture-specific code.
