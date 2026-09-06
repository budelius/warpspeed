# Changelog

Changes in this fork, relative to upstream [warpdotdev/Warp](https://github.com/warpdotdev/Warp).
Upstream's own release notes are not duplicated here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Fixed

- **Two terminal supervisors could end up serving one pipe, splitting the
  sessions between them.** `CreateNamedPipeW` does not fail on a name that
  already exists -- it adds another *server* instance, and a client is handed
  whichever is free. Two app instances starting at the same moment each found no
  supervisor, each spawned one, and both bound the name. Each then held sessions
  the other had never heard of: the detached-session picker listed half of them,
  and an attach landing on the wrong daemon reported the session missing and
  opened a fresh shell instead. Found on a dogfood machine running three daemons
  against two pipe names, two of them started a second apart.

  The listener's first instance now asks for `FILE_FLAG_FIRST_PIPE_INSTANCE`, so
  the second supervisor fails at bind and exits; its client's existing retry loop
  then connects to the one that won. This is what the Unix supervisor already did
  by refusing a socket another supervisor was serving.

### Added

- **Detach and attach a terminal session by hand.** "Detach session" in a pane's
  overflow menu and a tab's right-click menu hands that pane's shell -- and
  everything running under it, a CLI agent included -- to the session supervisor
  and closes the pane without killing anything. "Attach detached session" lists
  what the supervisor is holding and opens it in a new tab. Both are also
  command-palette entries (`workspace:detach_session`,
  `workspace:attach_detached_session`) with no default keybinding.

  The detach carries the pane's screen as an ANSI repaint, so a session that
  comes back into a fresh tab paints what it was showing rather than starting
  blank; a reattached *restored* pane still suppresses that repaint, because it
  has already drawn its own scrollback from sqlite. Sessions detached at app
  quit had been handed over with a blank placeholder screen and no metadata --
  the screen and the shell, directory and title now travel with them, which is
  also what makes the picker able to name them.

  Gated on `PersistentTerminalSessions`. The menu entries and palette commands
  hide themselves where no supervisor can be reached, rather than failing when
  chosen.

- **Quitting Warp no longer ends a session that still has work in it.** With
  `PersistentTerminalSessions` on, app exit asks every window what it is holding
  and keeps each pane that has a tracked CLI agent or a foreground command still
  running -- with its screen -- rather than only the panes of a launch-config
  session window. A pane sitting at a prompt is still ended, which is what
  quitting has always done. Restoring the window on the next launch reattaches
  what survived. A launch-config session window keeps every session as before,
  running or not, and now hands its screens over rather than a blank placeholder.

- **A Unix and macOS backend for the v2 session supervisor**, so detach, attach
  and the listing behave there as they do on Windows. The supervisor runs as a
  double-forked, `setsid`-ed process that outlives Warp; it speaks the same
  protocol over an `AF_UNIX` socket, and lends a session's PTY leader descriptor
  over `SCM_RIGHTS` where Windows duplicates a handle. Above the transport seam
  nothing differs between the two.

  This supersedes the v1 supervisor (`local_tty/server/persistent.rs`), which
  kept sessions alive across app exit but spooled their output as raw bytes --
  correct for line-oriented output, visibly wrong for a reattached `htop`, `vim`
  or Claude Code TUI -- and could not enumerate what it was holding. v1 remains
  the path when the flag is off, and its socket keeps its own name, so an old
  daemon left running after an upgrade holds its own sessions until its TTL
  reaps them rather than being spoken to in a protocol it does not know. With
  the flag on, Warp no longer starts the v1 daemon at all.

### Fixed

- **Dragging a window's edge pinned every open shell's CPU.** A drag relaid the
  terminal out every frame, and each of those frames sent its PTY a resize --
  even the frames that only changed the pane's pixel size and left the column
  and row counts alone. ConPTY is told nothing but columns and rows, so those
  resizes said nothing new, but each one still cost the console host a full
  buffer reflow and the shell a prompt repaint. With a dozen sessions open, one
  drag was a dozen of those per frame; PowerShell sessions in particular ran hot
  enough to throttle the machine.

  The Windows PTY now drops a resize whose column and row counts it already has,
  and the PTY event loop applies only the newest size in a batch instead of
  every size it was handed. A refresh -- the deliberate re-announcement that
  gets an alt-screen program's attention -- still goes through untouched. Unix
  is unchanged: `winsize` carries the pane's pixel dimensions, so every resize
  there does say something new.

- **A shell could not start at all when Warp ran inside a job object that
  forbids breakaway.** The Windows PTY spawn asked for `CREATE_BREAKAWAY_FROM_JOB`
  and treated the refusal as fatal, so `CreateProcessW` returned
  `ERROR_ACCESS_DENIED` and no shell appeared -- under a test runner, a CI agent,
  or any terminal that makes such a job. It now falls back to starting inside the
  job, which is what `supervised_pty`, `daemon_spawn` and `command::blocking`
  already did. A shell that dies with the job beats no shell.

### Changed

- The git branch, branch status and diff stats chips no longer run shell commands
  on a repository the libgit2-backed `GitRepoStatusModel` already reports on. All
  three took their values from that model and *also* from a shell generator whose
  result the model then overwrote, so every run was discarded work -- and a
  PowerShell process, on Windows. Where the shell could not run at all, a closed
  pane left the chips on a dead session and each refresh logged a warning, marked
  the chip failed and cleared a value the model had already filled: three logs
  from 2026-09-04 hold 3,108, 4,905 and 12,591 of those warnings, in bursts of
  hundreds inside a single second.

  This also closes the second shell path the libgit2 dropdown work left open.
  That change stopped the dropdown *refresh* from shelling out, but the chip's
  on-click generator still ran after every value fetch, so `git branch
  --sort=-committerdate` and `git worktree list --porcelain` kept running whether
  or not anyone opened the dropdown.

  Remote sessions are unchanged. Every chip value is carried over the wire; only
  the branch list is not, and the fallback that fetches it reaches the on-click
  generator by a path this guard does not sit on.

- Test-suite hygiene, chasing 36 failures on a developer machine down to the 14
  that need a Windows privilege this repo cannot grant itself. Each was a test
  reading something the machine happened to have, or something another test left
  behind:
  - `SessionInfo::new_for_test` left `histfile` unset, which is not the hermetic
    choice it looks like -- the session falls back to the shell's *real* history
    file, so history assertions passed on a clean CI runner and failed on a
    machine that had been used. It now names a file that does not exist.
  - 29 tests flipped feature flags with `FeatureFlag::set_enabled`, which is
    process-global and never undone, so every later test in the binary inherited
    them. They now use the thread-local, RAII-scoped `override_enabled` the
    overrides module prescribes.
  - `TestCommandExecutor` ran PowerShell as `pwsh` only. Where PowerShell 7 is
    not installed every "what commands exist" lookup returned "program not
    found", which reads as a machine with no `git` on it. It now falls back to
    `powershell`.
  - The experiments, telemetry and worktree-sidecar tests asserted on
    process-global totals, a shared event queue, and an un-abbreviated path --
    each true only in a particular running order, or only where the temp
    directory sits outside `$HOME`, which on Windows it does not.
  - The CLI-agent image-drop test waited on an off-thread read plus an exclusive
    OS clipboard using a scheduler-tick budget, and failed false-red under load.
    It now waits in wall clock, which is what `assert_eventually_within` exists
    for.

## [0.1.0] - 2026-09-04

First tagged release of this fork. Everything below accumulated since the fork
point; upstream's own release notes are not duplicated here.

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

[Unreleased]: https://gitlab.com/martin-wieser/warpspeed/-/compare/v0.1.0...dev
[0.1.0]: https://gitlab.com/martin-wieser/warpspeed/-/tags/v0.1.0
