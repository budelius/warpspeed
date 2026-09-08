# A forge layer for GitHub, GitLab and Gitea

**Status:** design
**Date:** 2026-09-08
**Scope:** pull request and repository lookups go through a forge client that speaks each host's
HTTP API, authenticated from credentials already on the machine, instead of shelling out to `gh`.

## Problem

Warp's only forge integration is the GitHub CLI. `get_pr_for_branch` and `get_repository_info` run
`gh pr view` and `gh repo view` through `run_gh_command` (`app/src/util/git.rs:811`), and PR creation
runs `gh pr create --fill`. Three consequences follow.

**It spawns a process per lookup.** `LocalGitHubRepoModel` refetches the PR on creation and on every
branch change, so a `gh` process runs whenever the branch moves.

**It only speaks GitHub.** `GitHubRepoModel`, `PrInfo` and the model that drives them assume one
forge. A repository on GitLab or Gitea gets nothing. This is not hypothetical for this repository,
which has two remotes on two forges:

```
origin  https://github.com/warpdotdev/Warp.git
gitlab  git@gitlab.com:martin-wieser/warpspeed.git
```

So "which forge is this repository on" is not a repository-level property. It is per remote, and an
action has to name the remote it targets.

**It depends on `gh` being installed and logged in.** `user_facing_git_error` already has dedicated
copy for "GitHub CLI not authenticated" (`git_dialog/mod.rs:171`), which is a workaround for a
dependency Warp does not need.

### Where the credentials actually are

Reading `gh`'s config file does not work. On the machine this was designed against, `hosts.yml`
holds no token at all:

```yaml
github.com:
    git_protocol: https
    users:
        budelius:
    user: budelius
```

Modern `gh` stores the token in the OS keyring instead. Windows Credential Manager on that machine
holds `gh:github.com:<user>`, and alongside it `git:https://github.com`,
`git:https://gitlab.com`, `git:https://oauth2@gitlab.com`, and a self-hosted
`git:https://git-tm.tourist-mobile.com`. `CredReadW` is an API call, not a subprocess, so reading
them satisfies the constraint that started this program.

`app/src/ai/agent_sdk/driver/git_credentials.rs` already documents the on-disk formats for both `gh`
and `glab`, because it *writes* them for cloud agents. It writes only to the Unix paths, and its
credentials come from the Warp server's `taskGitCredentials` query, scoped to a cloud task — so it
is a source of format knowledge, not of local tokens.

## Goals

- `PrInfo` and repository info for the current branch come from the forge's HTTP API, with no
  process spawned.
- GitHub, GitLab and Gitea are all supported, including self-hosted instances of the latter two.
- A host's forge kind is discovered rather than configured, and remembered.
- A token is found without the user configuring anything, when one is already on the machine.
- Nothing regresses for a user with `gh` installed and authenticated.

## Non-goals

- **Push and fetch.** `git2` is declared `default-features = false` with only `vendored-libgit2`
  (`app/Cargo.toml:321`), so no HTTPS or SSH transport is compiled in. Enabling that feature and
  driving `RemoteCallbacks::credentials` from this layer's credential chain is the obvious follow-on,
  and it is not this spec.
- **The 29 shell sites in `diff_state/local.rs`.** Unchanged and deliberately so; see the
  `util/git.rs` spec.
- **Turning on `GitOperationsInCodeReview`.** That is the last spec in this program, and it should
  follow this one rather than lead it.
- **Writing credentials.** This layer reads. It never creates, refreshes, or stores a token on the
  user's behalf.

## Design

### 1. Module layout

```
app/src/forge/
  mod.rs          ForgeKind; Remote URL → Host; the public entry points
  client.rs       trait ForgeClient
  github.rs       REST v3
  gitlab.rs       REST v4      (merge request ≈ pull request)
  gitea.rs        REST v1
  detect.rs       probe and cache: host → ForgeKind
  credentials.rs  trait CredentialStore and the resolution chain
  credentials/
    settings.rs   per-host override from Warp settings
    managed.rs    PAT held in the managed_secrets crate
    os_windows.rs CredReadW
    os_macos.rs   Keychain
    os_linux.rs   libsecret
```

### 2. `ForgeClient` is the seam

```rust
#[async_trait]
pub trait ForgeClient: Send + Sync {
    async fn pr_for_branch(&self, repo: &RepoRef, branch: &str) -> Result<Option<PrInfo>>;
    async fn create_pr(&self, repo: &RepoRef, req: &CreatePrRequest) -> Result<PrInfo>;
    async fn repository_info(&self, repo: &RepoRef) -> Result<RepositoryInfo>;
}
```

`RepoRef` carries host, owner and name, parsed from the remote URL — which means SSH
(`git@gitlab.com:owner/name.git`) and HTTPS (`https://github.com/owner/name.git`) forms both have to
parse, since this repository uses one of each.

`PrInfo` is unchanged (`util/git.rs:726`). It already carries `number`, `url`, `state`, `draft` and
`base_branch`, and all three forges supply every field; GitLab's `iid` maps to `number`. Keeping the
type means `GitHubRepoModel`'s consumers — prompt chips, code review, agent context — do not change.

### 3. `GitHubRepoModel` becomes `ForgeRepoModel`

The existing model already dispatches Local and Remote backends behind one handle
(`github_repo_model/mod.rs`). It gains a second dimension: which forge. The Local backend stops
running `gh` and calls a `ForgeClient` chosen by the remote's host; the Remote backend is untouched,
because a remote session's PR info still arrives over the wire.

The rename is not cosmetic. Every consumer currently reads a type called `GitHubRepoModel` for a
repository that may be on GitLab, and leaving that name would make the single-forge assumption
invisible again.

### 4. The credential chain

One trait, three implementations, consulted in order. First hit wins.

1. **Settings override.** An explicit per-host entry in Warp settings. Always wins, so a user can
   correct a wrong guess or point at a second account.
2. **`managed_secrets`.** A PAT the user gave Warp directly. Self-contained and portable.
3. **The OS store.** `gh:<host>:<user>`, then `git:https://<host>`. Zero configuration, and it
   reuses whatever the user already logged into.

A miss is not an error. It resolves to "unauthenticated", and the panel shows a sign-in affordance
rather than raising a toast — the same distinction the code review view already draws between
`NoRepoFound` and `Error`.

`Win32_Security_Credentials` is not in the app's `windows` feature list (`app/Cargo.toml:403`) and
has to be added. macOS and Linux get their own implementations behind the same trait; a platform
without one returns no credential rather than failing to build.

### 5. Detection probes, then caches

A hostname does not say what software runs on it. `github.com` and `gitlab.com` are known, and
anything else is probed: `GET /api/v4/version` answers on GitLab, `GET /api/v1/version` on Gitea,
and GitHub Enterprise answers `GET /api/v3/`. Whichever responds decides, and the answer is cached
per host.

**A failed probe caches a negative with a TTL.** This is the mistake that produced the 12,591-warning
storm in the chips: an operation that cannot succeed, re-driven on every branch change. A host that
does not answer must not be re-probed on the next keystroke.

Probing sends an unauthenticated request to a host taken from the user's git remote. That is a host
the user already talks to, but it is still an outbound request the user did not explicitly ask for,
so it happens on first use of a forge feature for that repository — not at startup, and not for a
repository whose panel nobody opened.

### 6. Error model

Three outcomes, and they are not interchangeable:

- **Unauthenticated** — no credential resolved. Offer sign-in.
- **Unsupported** — the host answered no probe. Say so once and stop asking.
- **Failed** — the API returned an error or the network did. Surface it, and let the caller retry.

`user_facing_git_error`'s `gh`-specific copy (`git_dialog/mod.rs:162-171`) is replaced by copy that
names the forge rather than the CLI.

## Staging

Each stage is independently landable and independently useful.

1. **`ForgeKind`, `RepoRef`, remote URL parsing.** Pure functions, no network, no credentials.
2. **The credential chain**, with the Windows store first. Testable against a fake store.
3. **Detection**, probe and cache, against a stub HTTP client.
4. **The GitHub client**, replacing `gh pr view` and `gh repo view`. This is the point where
   behavior changes for existing users, so it lands with the fallback still in place.
5. **GitLab and Gitea clients.**
6. **PR creation**, replacing `gh pr create --fill`.
7. **Retire the `gh` path** once the clients have shipped.

## Testing

- URL parsing: SSH and HTTPS forms, a port, a path with more than two segments, a missing `.git`
  suffix, and a self-hosted host. Table-driven.
- The credential chain: a fake store per layer, asserting order, that a miss falls through, and that
  a miss at every layer is "unauthenticated" rather than an error.
- Detection: a stub HTTP client returning each forge's version response, plus a host that answers
  nothing, asserting the negative is cached and the host is not re-probed within the TTL.
- Each client: recorded response fixtures for a PR that exists, no PR, and an API error. No test
  touches the network or the real keyring.

## Risks

- **Probing an unexpected host.** Mitigated by probing only on first use of a forge feature for that
  repository, never at startup.
- **A token with more scope than the operation needs.** Reading `git:https://<host>` yields whatever
  the user's git credential is, which may be broad. This layer never writes it anywhere and never
  sends it to a host other than the one it was stored for. That last point is a rule, not a
  consequence: the client must refuse to send a credential to any host but its own.
- **Rename churn.** `GitHubRepoModel` → `ForgeRepoModel` touches every consumer. Landing it as its
  own commit, separate from behavior changes, keeps the diff reviewable.
