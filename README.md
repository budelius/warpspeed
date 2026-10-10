# WarpSpeed by STOZN

A fork of [warpdotdev/Warp](https://github.com/warpdotdev/Warp), used for work on
terminal startup and session-restore behaviour.

## What this fork adds

- **A `warpspeed` build target.** Upstream's `dev`, `local`, `stable`, and `preview`
  binaries resolve their channel config at runtime from a private generator that
  needs SSH access to `warpdotdev/warp-channel-config`, and panic at startup
  without it. `oss` runs standalone but keeps dogfood flags off. `warpspeed`
  carries an inline config *and* enables dogfood flags, so in-progress features
  are actually reachable. It also takes a distinct `AppId` that varies with
  elevation and `WARPSPEED_PROFILE`, so it runs alongside an installed Warp
  instead of sharing its settings, database, and session state.
- **Lazy session restore.** Restored tabs defer starting their shells until activated,
  so opening a large saved session forks shells for the active tab rather than every
  pane. The reduction in shell count has not been measured.
- **Self-hosted AgentCtrl sessions.** `warpspeed agent-ctrl` discovers, starts,
  follows, resumes, messages, closes, and stops staging agents through the
  authenticated STOZN dashboard. This path does not use Warp accounts, Oz,
  Session Sharing, Remote Control, hosted transcripts, or Warp API keys.

## Building

```bash
export PROTOC=/path/to/protoc          # required by workspace build scripts
cargo run -p warp --bin warpspeed --features tui
```

On Windows, install WarpSpeed for routine use instead of launching it from Cargo's build directory:

```powershell
powershell -ExecutionPolicy Bypass -File .\script\windows\install-warpspeed.ps1
```

The installer stages only the `warpspeed` release binary under `target/warpspeed-merge`, installs a versioned copy under `%LOCALAPPDATA%\Programs\WarpSpeed`, and updates the Start Menu shortcuts and `~/.local/bin/warpspeed.cmd`. This keeps active executables out of `target/`, so they cannot block later builds. **Restart window in new process** switches an open window to the current installed version. Pass `-SKIP_BUILD` to install an already staged build.

Installing never disturbs a running WarpSpeed. Each instance keeps running from the version directory it launched with, and only new launches pick up the newly installed version -- the launcher resolves `current.txt` at launch time. Old version directories are pruned only when every running instance can be identified; if one will not report its executable path (an elevated instance, typically), the installer says so and leaves every version in place.

File-backed session configs compose reusable tab configs into one persistent, lazily restored window. On Windows, definitions live under `%APPDATA%\warp\WarpSpeed\data\session_configurations`; full runtime snapshots live under `%LOCALAPPDATA%\warp\WarpSpeed\data\session_config_sessions`. Open one directly with a URL argument such as `warplocal://launch/JustVIBE.toml`. YAML launch configurations remain static templates and no longer restore an evolving runtime snapshot.

## Self-hosted managed agents

Use the graphical **Sign in with STOZN** flow. WarpSpeed opens a one-time dashboard approval page, shows a confirmation code that must match the page, and stores the resulting expiring desktop credential in its own state directory (DPAPI-protected for the current OS user on Windows); no Warp account or prior `stozn auth` command is required. `STOZN_TOKEN` and the legacy `~/.stozn.json` remain non-graphical migration fallbacks.

Create or select the task in the STOZN Dashboard, choose the configured agent there, and start the managed session. After AgentCtrl accepts the launch, the dashboard opens a strict `warpspeed://agent-ctrl/attach?...` deep link. WarpSpeed validates the compiled staging environment, fleet, and job ID, then opens only that session's normalized event viewer with bounded `send`, `close`, `stop`, and `detach` controls. WarpSpeed deliberately has no duplicate task picker or task-creation UI.

The lower-level CLI remains available for automation and recovery:

```bash
warpspeed agent-ctrl open <job-id>
warpspeed agent-ctrl list
warpspeed agent-ctrl start --agent staging-claude-interactive --task <stozn-task-id> --interactive
warpspeed agent-ctrl attach <job-id>
warpspeed agent-ctrl input <job-id> "Inspect the failing test"
warpspeed agent-ctrl close <job-id>
warpspeed agent-ctrl pause <job-id>
warpspeed agent-ctrl resume <job-id>
warpspeed agent-ctrl stop <job-id>
```

`open` and a following `attach` use a full-screen conversation UI when stdin and stdout are connected to a terminal. Type a follow-up and press Enter, scroll with the arrow or page keys, close agent input with Ctrl-D, request a stop with Ctrl-X, and detach without stopping the run with Esc or Ctrl-C. Type `/help` or press F1 for run-control and local-shell commands. `/shell` or Ctrl-O opens an interactive local shell; `!<command>`, `/ssh`, and `/tmux` suspend the UI and run against the local terminal, returning to AgentCtrl when the program exits. Use `//` to send an agent message that begins with `/`. Use `attach --once`, `--json`, or redirected input/output for the line-oriented interface.

Every new managed session requires a dashboard-validated STOZN task. The dashboard endpoint, environment, and fleet identity are compiled/operator-owned; callers cannot supply controller URLs or runtime overrides. Local bookmarks contain only safe identifiers, labels, cursors, state, and timestamps. Closing `attach` never stops the Kubernetes process.

## Recovering CLI agent sessions

A pane that was running a CLI agent records which agent it was, in which shell and directory, and the session ID it reported. Restoring the window -- including on a fresh boot -- puts the resume command back in that pane's input, ready to run: `claude --resume <id>`, `codex resume <id>`, `vibe --resume <id>` and `pi --session <id>`. Other agents are recorded but not resumed, because their CLIs cannot be told to open a session by ID.

The same information is written to `agent-sessions.md` and `agent-sessions.json` in the state directory (`%LOCALAPPDATA%\warp\WarpSpeed\data` on Windows) every time window state is saved, so a session is still recoverable by hand when its pane is not. Warp learns a session ID from the agent's own Warp plugin; an agent running without that plugin is recorded with no ID and cannot be resumed.

On Linux, install build dependencies first with `./script/linux/install_build_deps`.

## Branches

| Branch | Contents |
| --- | --- |
| `main` | Upstream snapshot `19dc505`, plus this fork's build target |
| `dev` | Integration branch; CI lints merge requests and pushes here |
| `feature/*` | Work in progress |

Note: this repository was created from a shallow clone, so history begins at
upstream `19dc505` rather than at Warp's root commit.

## License and attribution

Licensing is unchanged from upstream: dual-licensed under AGPL-3.0 and MIT. See
[`LICENSE-AGPL`](LICENSE-AGPL) and [`LICENSE-MIT`](LICENSE-MIT).

Warp is a product of Warp Dev, Inc. This fork is independent, and is neither
affiliated with nor endorsed by Warp Dev, Inc. All upstream copyright and
attribution is retained; the upstream README follows below unmodified.

---

<a href="https://www.warp.dev">
    <img width="1024" alt="Warp Agentic Development Environment product preview" src="https://github.com/user-attachments/assets/9976b2da-2edd-4604-a36c-8fd53719c6d4" />
</a>
&nbsp;
<p align="center">
  <a href="https://www.warp.dev"><img height="20" alt="Built with Warp" src="https://raw.githubusercontent.com/warpdotdev/brand-assets/main/Github/Built-With-Warp-Export@2x.png" /></a>
  &nbsp;
  <a href="https://oz.warp.dev"><img height="20" alt="Powered by Oz" src="https://raw.githubusercontent.com/warpdotdev/brand-assets/main/Github/Powered-By-Oz-Export@2x.png" /></a>
</p>

<p align="center">
  <a href="https://www.warp.dev">Website</a>
  ·
  <a href="https://www.warp.dev/code">Code</a>
  ·
  <a href="https://www.warp.dev/agents">Agents</a>
  ·
  <a href="https://www.warp.dev/terminal">Terminal</a>
  ·
  <a href="https://www.warp.dev/drive">Drive</a>
  ·
  <a href="https://docs.warp.dev">Docs</a>
  ·
  <a href="https://www.warp.dev/blog/how-warp-works">How Warp Works</a>
</p>

> [!NOTE]
> OpenAI is the founding sponsor of the new, open-source Warp repository, and the new agentic management workflows are powered by GPT models.

<h1></h1>

## About

[Warp](https://www.warp.dev) is an agentic development environment, born out of the terminal. Use Warp's built-in coding agent, or bring your own CLI agent (Claude Code, Codex, Gemini CLI, and others).

## Installation

You can [download Warp](https://www.warp.dev/download) and [read our docs](https://docs.warp.dev/) for platform-specific instructions.

## Warp Contributions Overview Dashboard

Explore [build.warp.dev](https://build.warp.dev) to:
- Watch thousands of Oz agents triage issues, write specs, implement changes, and review PRs
- View top contributors and in-flight features
- Track your own issues with GitHub sign-in
- Click into active agent sessions in a web-compiled Warp terminal

## Oz for OSS

Maintaining a popular open-source project? [Apply for Oz credits](https://tally.so/r/LZWxqG) to explore [Oz for OSS](https://github.com/warpdotdev/oz-for-oss).

Oz for OSS is our partner program for bringing the same agentic open-source management workflows used in this repository to select partner repositories. We work directly with maintainers to implement workflows for issue triage, PR review, community management, and contributor coordination in a way that fits each project.

## Licensing

Warp's UI framework (the `warpui_core` and `warpui` crates) are licensed under the [MIT license](LICENSE-MIT).

The rest of the code in this repository is licensed under the [AGPL v3](LICENSE-AGPL).

## Open Source & Contributing

Warp's client codebase is open source and lives in this repository. We welcome community contributions and have designed a lightweight workflow to help new contributors get started. For the full contribution flow, read our [CONTRIBUTING.md](CONTRIBUTING.md) guide.

> [!TIP]
> **Chat with contributors and the Warp team** in the [`#oss-contributors`](https://warpcommunity.slack.com/archives/C0B0LM8N4DB) Slack channel — a good place for ad-hoc questions, design discussion, and pairing with maintainers. New here? [Join the Warp Slack community](https://go.warp.dev/join-preview) first, then jump into `#oss-contributors`.

### Issue to PR

Before filing, [search existing issues](https://github.com/warpdotdev/warp/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc) for your bug or feature request. If nothing exists, [file an issue](https://github.com/warpdotdev/warp/issues/new/choose) using our templates. Security vulnerabilities should be reported privately as described in [CONTRIBUTING.md](CONTRIBUTING.md#reporting-security-issues).

Once filed, a Warp maintainer reviews the issue and may apply a readiness label: [`ready-to-spec`](https://github.com/warpdotdev/warp/issues?q=is%3Aissue+is%3Aopen+label%3Aready-to-spec) signals the design is open for contributors to spec out, and [`ready-to-implement`](https://github.com/warpdotdev/warp/issues?q=is%3Aissue+is%3Aopen+label%3Aready-to-implement) signals the design is settled and code PRs are welcome. Anyone can pick up a labeled issue — mention **@oss-maintainers** on an issue if you'd like it considered for a readiness label.

### Building the Repo Locally

To build and run Warp from source:

```bash
./script/bootstrap   # platform-specific setup
./script/run         # build and run Warp
./script/presubmit   # fmt, clippy, and tests
```

See [AGENTS.md](AGENTS.md) for the full engineering guide, including coding style, testing, and platform-specific notes.

## Joining the Team

Interested in joining the team? See our [open roles](https://www.warp.dev/careers).

## Support and Questions

1. See our [docs](https://docs.warp.dev/) for a comprehensive guide to Warp's features.
2. Join our [Slack Community](https://go.warp.dev/join-preview) to connect with other users and get help from the Warp team — contributors hang out in [`#oss-contributors`](https://warpcommunity.slack.com/archives/C0B0LM8N4DB).
3. Try our [Preview build](https://www.warp.dev/download-preview) to test the latest experimental features.
4. Mention **@oss-maintainers** on any issue to escalate to the team — for example, if you encounter problems with the automated agents.

## Code of Conduct

We ask everyone to be respectful and empathetic. Warp follows the [Code of Conduct](CODE_OF_CONDUCT.md). To report violations, email warp-coc at warp.dev.

## Open Source Dependencies

We'd like to call out a few of the [open source dependencies](https://docs.warp.dev/help/licenses) that have helped Warp to get off the ground:

- [Tokio](https://github.com/tokio-rs/tokio)
- [NuShell](https://github.com/nushell/nushell)
- [Fig Completion Specs](https://github.com/withfig/autocomplete)
- [Warp Server Framework](https://github.com/seanmonstar/warp)
- [Alacritty](https://github.com/alacritty/alacritty)
- [Hyper HTTP library](https://github.com/hyperium/hyper)
- [FontKit](https://github.com/servo/font-kit)
- [Core-foundation](https://github.com/servo/core-foundation-rs)
- [Smol](https://github.com/smol-rs/smol)
