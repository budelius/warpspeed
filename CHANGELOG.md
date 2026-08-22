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

### Changed

- Merged upstream through `e2a08021`. The `warpspeed` target still builds against
  it unchanged.

### Notes

- History extends to Warp's root commit. The repository began as a shallow clone,
  but GitLab rejects shallow pushes, so it was unshallowed before publishing.
- `feature/launch-config-shell-improvements` carries lazy session restore and is
  not yet merged. Its shell-count reduction has not been measured.
