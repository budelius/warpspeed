# Changelog

Changes in this fork, relative to upstream [warpdotdev/Warp](https://github.com/warpdotdev/Warp).
Upstream's own release notes are not duplicated here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `warpspeed` build target: a standalone binary with an inline channel config and
  dogfood flags enabled, taking a distinct `AppId` that varies with elevation and
  `WARPSPEED_PROFILE` so it runs alongside an installed Warp.
- GitLab CI targeted at GitLab.com shared runners: an automatic `fmt` gate on
  merge requests and `dev` (compiles nothing, so it needs no system packages and
  no LFS payload), plus manual `clippy` and `build:warpspeed` jobs that build the
  full dependency graph. Config validated with GitLab's CI lint; the compile jobs
  have not yet completed a run.

### Changed

- Merged upstream through `e2a08021`. The `warpspeed` target still builds against
  it unchanged.

### Notes

- History extends to Warp's root commit. The repository began as a shallow clone,
  but GitLab rejects shallow pushes, so it was unshallowed before publishing.
- `feature/launch-config-shell-improvements` carries lazy session restore and is
  not yet merged. Its shell-count reduction has not been measured.
