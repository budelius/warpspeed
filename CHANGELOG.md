# Changelog

Changes in this fork, relative to upstream [warpdotdev/Warp](https://github.com/warpdotdev/Warp).
Upstream's own release notes are not duplicated here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `warpspeed` build target: a standalone binary with an inline channel config and
  dogfood flags enabled, taking a distinct `AppId` that varies with elevation and
  `WARPSPEED_PROFILE` so it runs alongside an installed Warp.
- GitLab CI: `lint` on merge requests and `dev`, tag-gated `publish` producing a
  `warpspeed` release binary as a job artifact. Unverified — has not yet run.

### Notes

- Forked from upstream `19dc505` via a shallow clone; history does not extend
  past that commit.
- `feature/launch-config-shell-improvements` carries lazy session restore and is
  not yet merged. Its shell-count reduction has not been measured.
