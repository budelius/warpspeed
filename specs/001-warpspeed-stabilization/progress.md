# Implementation Progress

Branch: `fix/macos-local-build`, fast-forwarded to `origin/dev` (`ffe4ad05`) before the latest build.
The attach MR and Windows WIP snapshot remain separate; neither was bulk-merged.

## Mac build/install slice

Implemented:

- Short macOS legacy persistent-server socket names with a protected temporary-directory fallback
  when the state path exceeds the Unix socket limit. Hashes distinguish profiles/channels; Linux
  names remain unchanged. The client validates the selected path before connecting.
- Five path regressions, including a real bind at the fallback and refusal of symlink directories.
- Repaired Unix supervisor fixtures missing `bootstrap`/`metadata` fields.
- `script/macos/bundle --no-dmg`, restricted to self-signing/no-signing workflows.
- `script/macos/install-warpspeed`: native OSS build, signature/identity validation, same-filesystem
  staging, backup/rollback, and refusal to replace running apps or symlink destinations.
- Five installer tests using real signed macOS fixtures.
- Borrowed menu labels correctly in the Mac app menu.

The pre-existing removal of the OSS `PersistentLaunchConfigSessions` override is preserved separately.
Its intended product behavior remains an open decision (T010), not an implicit recommendation.

The earlier first-slice artifact was installed and launched. A **newer**, signed artifact containing
`origin/dev` plus local work is now at `target/debug/bundle/osx/WarpOss.app`. It has not replaced the
running `/Applications/warpseed.app`; no existing sessions were terminated. Exact-commit installation
and full shell/session acceptance remain open (T009).

## Local Warp import

Imported 17 files from `~/.warp` to `~/.warp-oss`: 14 launch YAMLs and 3 tab TOMLs. Conflicting files
were preserved using `warp-import-*` filenames and `Warp / ` display names. Backups and the migration
report are under `~/.warp-config-backups/20260917-134653-warp-import` (not committed).

General settings were not migrated: Python's TOML parser rejected the source `settings.toml`, and a
subsequent search found no launch/shell/startup-related lines. No credentials or runtime hook captures
are included in these source changes.

## Pane-maximize investigation

Added `test_maximize_pane_updates_terminal_size_without_window_resize`, registered in both integration
runners. It splits right, starts a Python SIGWINCH probe, maximizes/restores without resizing the
window, and checks both terminal-view columns and the child process's reported PTY columns.

It passes in the regular harness, under nextest, and with
`WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1` on macOS. The first failing draft incorrectly dispatched
an action while its target view was borrowed; deferred dispatch corrected the test. That failure was
**not** a reproduced product bug. A speculative terminal notification change did not resolve that
failure and was reverted.

The reported stale rendered viewport is **not fixed or conclusively reproduced**. The passing checks
cover dimensions and SIGWINCH delivery, not pixel-level repaint correctness or a particular Pi/Claude
screen. Next obtain the affected program and a repeatable visual sequence.

## Verification

Passed:

- `./script/format` and `./script/format --check`.
- All three prescribed Clippy commands (workspace excluding completer, warp, and completer).
- Presubmit inline-test, clang-format, factory-files, and WGSL checks.
- Focused persistent-server/Unix transport nextest selection: **20 passed**.
- macOS signed-fixture installer tests: **5 passed**.
- Pane-maximize integration check, including the real macOS display run.
- Native debug OSS `--selfsign --no-dmg` bundle and `codesign --verify --deep --strict`.

Resolved existing local submission blockers without suppressing lints: used an attached-PTY type
alias, switched session CLI clocks to `instant::Instant`, corrected literal formatting, applied the
repository formatter, and moved four inline test modules into sibling test files without changing
the tests. Installed clang-format and the repository-pinned wgslfmt to run the remaining checks.

Full `./script/presubmit` was run. Its workspace nextest stage reported **11,821 passed**, **4 failed**,
**5 timed out**, and **91 skipped** (11,830 run; one pass reported leaky):

- `test_git_prompt` failed.
- `test_a_closing_window_hands_its_session_to_the_supervisor` failed; logs show the newer Unix
  supervisor cannot connect at the integration harness's long state-directory socket path.
- `test_detach_and_attach_a_session` failed.
- `drive_path_round_trips_through_the_forward_converter` failed converting a Windows path on macOS.
- Five SSH integration cases timed out (`ash`, `sh`, wrapper `bash`, wrapper `zsh`, shell override).

These failures have not been resolved or proven unaffected by this branch. Later presubmit stages
were not reached. Full presubmit is **not green**; do not promote this branch as release-verified.
Logs remain outside the repository under `/tmp/warpspeed-mac-build/`.

Linux/Windows cloud verification was not started while the full local suite remains unresolved.
See [macOS CI options](macos-ci.md) for Xcode Cloud prerequisites and the direct GitLab-runner option.

## Resume installation

After quitting warpseed:

```sh
./script/macos/install-warpspeed --from-bundle "$PWD/target/debug/bundle/osx/WarpOss.app"
open /Applications/warpseed.app
```

Verify a usable shell, supervisor startup, representative migrated launches, and session lifecycle.
T009–T012 remain open; the passing resize probe does not close the visual repaint report.
