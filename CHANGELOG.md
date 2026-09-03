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

### Changed

- All `WarpSpeed` profiles, including elevated and `WARPSPEED_PROFILE` instances, now share one
  authored set of launch, session, and tab configurations while retaining isolated runtime state.
- Merged upstream through `e2a08021`. The `warpspeed` target still builds against
  it unchanged.

### Fixed

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
- The Windows installer no longer deletes a version directory a running WarpSpeed
  is using. It read each instance's path with `Get-Process`, which an elevated
  instance will not answer from a non-elevated installer -- and this fork's
  `AppId` varies with elevation precisely so elevated and normal instances run
  side by side. An unreadable instance looked like no instance, so its build was
  pruned out from under it. Paths now come from `Win32_Process` first, and any
  instance that still will not say leaves every version in place.

### Notes

- History extends to Warp's root commit. The repository began as a shallow clone,
  but GitLab rejects shallow pushes, so it was unshallowed before publishing.
- `feature/launch-config-shell-improvements` carries lazy session restore and is
  not yet merged. Its shell-count reduction has not been measured.
