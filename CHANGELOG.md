# Changelog

Changes in this fork, relative to upstream [warpdotdev/Warp](https://github.com/warpdotdev/Warp).
Upstream's own release notes are not duplicated here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `warpspeed` build target: a standalone binary with an inline channel config and
  dogfood flags enabled, taking a distinct `AppId` that varies with elevation and
  `WARPSPEED_PROFILE` so it runs alongside an installed Warp.
- GitLab CI on the self-hosted `obachan-staging-oss` runner: `fmt` and `clippy`
  on merge requests and `dev`, and `build:warpspeed` automatic on version tags,
  opt-in on `dev`. `fmt` compiles nothing, so it skips system packages and the
  LFS payload and finishes in about a minute. Config validated with GitLab's CI
  lint; `fmt` has completed a real run, the compile jobs have not.
- Windows 11 Snap Layouts on the maximize button. Warp draws its own caption buttons on an
  undecorated window, so Windows never saw the `HTMAXBUTTON` hit-test answer that offers the
  flyout. A window subclass now supplies it and replays the non-client mouse messages that follow,
  leaving the button's hover, press, and click behavior unchanged. Windows 10 is untouched.
- Persistent TOML sessions on Windows now keep their supervisor-owned shells and foreground
  processes alive across orderly window or app closure and reattach them on reopen. Claude Code
  resumes automatically from its saved session ID if the original PTY is unavailable.
- A CLI agent session registry at `agent-sessions.json` and `agent-sessions.md` in the state
  directory, rewritten on every window-state save. Each entry names the agent, its shell, its
  directory, its session ID and the command that reopens it, so a session can be recovered by
  hand when the pane it was in does not come back.

### Changed

- The git branch chip's dropdown is now enumerated with libgit2 instead of running
  `git branch` and `git worktree list` through the session's shell. The refresh runs on
  every branch change, not on click, so on Windows it spawned a PowerShell process each
  time -- measured at ~435 ms against ~16 ms for the same work in Git Bash -- to fill a
  dropdown that was usually never opened. The emitted shape is unchanged, so the existing
  parsing and its tests are untouched. Remote repositories keep the shell path: the branch
  list is not carried over the wire.
- All `WarpSpeed` profiles, including elevated and `WARPSPEED_PROFILE` instances, now share one
  authored set of launch, session, and tab configurations while retaining isolated runtime state.
- A pane's CLI agent is now recorded even when it never reported a session ID, because "the agent
  left no way back in" is itself the answer to why a session did not return. Codex, Mistral Vibe
  and Pi join Claude Code in resuming automatically; their syntax was verified against the shipped
  CLIs. Gemini and OpenCode are recorded but not resumed -- `gemini --resume` takes an index rather
  than a session ID, and `opencode session` cannot open one -- because a guessed flag starts a
  fresh session and scrolls away the ID it was meant to recover.
- Merged upstream through `e2a08021`. The `warpspeed` target still builds against
  it unchanged.

### Fixed

- A session reattach no longer draws a second copy of the screen. `render_screen` opened its
  repaint with `CSI 2J`, which on Warp's primary-screen grid preserves the visible rows as
  block scrollback rather than blanking them, so the repaint appended a frame instead of
  replacing one. Each row is now erased individually with `CSI K` after it is positioned,
  which leaves the block's real scrollback intact -- `CSI 3J` would have discarded it, and
  the snapshot does not carry it to restore.
- Arrow keys no longer stop reaching a foreground program when a block is still
  selected from an earlier click. `terminal_up`/`terminal_down` consulted the
  block selection before the running command, so a stale selection silently
  swallowed up/down and left interactive selectors -- a CLI agent's question
  prompt, `fzf`, `git add -p` -- impossible to navigate.
- A pane no longer goes blind when an in-band generator's terminating marker is
  lost. Capturing diverts every printable byte away from the grid, so the capture
  is now bounded and abandoned if no marker arrives, and its cursor arithmetic
  counts characters rather than bytes -- counting bytes could underflow or split
  a multi-byte character and panic the PTY reader thread.
- A generator whose bytes never reached the line editor no longer leaks the
  in-flight in-band count. The leak kept the input box visible, which stopped the
  terminal taking keyboard focus for the rest of the session.
- CLI agent sessions now survive a reboot. The resume descriptor was captured into each pane's
  snapshot and then dropped on the way to SQLite -- the write ignored it and the read hardcoded
  `None` -- so it only ever reached a window restarted in a new process, never one restored on a
  fresh boot, which is the case it was written for. It is now a column like any other. Sessions
  restored from a TOML session config were unaffected; those snapshots are JSON and always carried
  it.
- An installed version is no longer labelled with the tree the build finished in. The installer
  asked git for the revision and the dirty flag after building, and a release build takes a
  quarter of an hour -- long enough to commit, check out, or edit underneath it. Builds came out
  marked `-dirty` for changes made while they compiled, and could name a revision they were never
  built from. Both are read before the build now, and the release notes generated from them are
  right again.
- The Windows installer no longer deletes a version directory a running WarpSpeed
  is using. It read each instance's path with `Get-Process`, which an elevated
  instance will not answer from a non-elevated installer -- and this fork's
  `AppId` varies with elevation precisely so elevated and normal instances run
  side by side. An unreadable instance looked like no instance, so its build was
  pruned out from under it. Paths now come from `Win32_Process` first, and any
  instance that still will not say leaves every version in place.

### Notes

- Both paths that can end the process when a window closes -- `on_should_close_window` and the
  window-process handoff -- now log the open window count and the branch they took. Added while
  investigating a report that closing one window of a multi-window process closes all of them;
  the log shows the `count == 1` guard is not what fires, so the cause is still open.

- History extends to Warp's root commit. The repository began as a shallow clone,
  but GitLab rejects shallow pushes, so it was unshallowed before publishing.
- Lazy session restore is merged. Its shell-count reduction has not been measured.
