# Kimi Code and DeepSeek Harness — Part A (WarpSpeed local) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kimi Code (`kimi`) and DeepSeek Harness (`dsh`) run in a WarpSpeed pane at the same level as claude, pi and codex: detected, branded, status-tracked through an out-of-band `warpspeed agent-event` channel, resumable (Kimi), listed in the session picker, with one-click hook/plugin installers.

**Architecture:** Two new `CLIAgent` variants flow through every existing per-agent table. Because Kimi hooks and the dsh Cordis plugin cannot write OSC 777 to the pane, each local pane's shell gets `WARP_AGENT_EVENT_TOKEN` + `WARP_AGENT_EVENT_BIN`; the hook/plugin runs `"$WARP_AGENT_EVENT_BIN" agent-event --format …`, which sends `Request::CliAgentEvent { token, notification }` over the existing remote-control pipe; the owning instance finds the pane by token and re-emits the notification into that pane's `ModelEventDispatcher` as a `ModelEvent::PluggableNotification` with the `warp://cli-agent` title, so everything downstream (listener registration, `CLIAgentSessionsModel`, footer, notifications, `transcript_path`) is byte-for-byte the OSC 777 path. The token is derived from the shell's supervisor `SessionId` with a per-profile secret, so a pane that adopts a supervised shell after detach/attach or a crash computes the same token the shell was started with.

**Tech Stack:** Rust (warp app crate, `warp_cli`, `warp_core`, `input_classifier`), `toml_edit` 0.25 (already a direct `app` dependency), `zstd` 0.13 (in `Cargo.lock`, made a direct `app` dependency), `sha2`/`hex`/`rand` (already direct `app` dependencies), TypeScript (Cordis plugin `@stozn/dsh-warp`).

## Global Constraints

- Worktree: `T:\projects\dev-ops\warp-kimi-dsh`, branch `feat/kimi-dsh-agents` (based on `dev`). Push to `gitea` and `gitlab`, never `origin`.
- Stage explicit paths only (`git add <path> …`); never `git add -A` / `git add .` — other sessions edit sibling trees.
- Every commit message ends with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- AGENTS.md style: context parameter named `ctx` and last (closure last if any); no `_`-prefixed unused params in new non-trait functions (remove them); exhaustive `match` (no `_` arm when matching `CLIAgent`); comments only for non-obvious "why"; tests in `${file}_tests.rs` / `mod_tests.rs` included with `#[cfg(test)] #[path = "…"] mod tests;`; inline format args; imports at file top.
- Display names exactly: "Kimi Code", "DeepSeek Harness". Notification messages exactly: "Notification from Kimi Code", "Notification from DeepSeek Harness".
- Command prefixes exactly: Kimi `["kimi"]`; dsh `["dsh"]` plus `npx [-y|--yes] @deepseek-ai/dsh[@version] …`.
- Kimi resume: `kimi --session <id>`; dsh resume: none.
- Kimi `[[hooks]]` entries may contain only `event`, `matcher`, `command`, `timeout`; `timeout = 5`.
- Feature flags `KimiNotifications`, `DshNotifications` in `DOGFOOD_FLAGS` (WarpSpeed enables DOGFOOD + PREVIEW unconditionally, `app/src/bin/warpspeed.rs`).
- `agent-event` always exits 0, silently, within 2 s.
- Windows is the primary platform; `WarpSpeed` is a console-subsystem binary (`release_bundle` is not enabled), so `cmd.exe` hooks wait for it and pass stdin.
- Never restart or kill running WarpSpeed processes; install with `script/windows/install-warpspeed.ps1` and verify in new windows.
- Known noise: ~22 `StoznAuthManager` test failures pre-exist on `dev`; a test run is green when the only failures are those.

## Decisions taken while grounding the spec in code (read before starting)

1. **Token derivation instead of a stored random token.** The spec asks for a random 128-bit token "kept with the pane's session so it survives detach/attach". The supervisor wire (`SessionMetadata`, protocol v6) carries no such field, an attaching pane gets a brand-new pane UUID (`pane_group/mod.rs:3565`), and a pane whose previous client crashed gets no metadata at all (`SessionBootstrap::for_orphaned_local_shell`). Every local pane, however, has a stable `SessionId` that both the spawning and the adopting pane know (`active_session_id` in `terminal_manager.rs:1711`). So the token is `hex(sha256(secret ‖ session_id_be)[..16])` — 128 bits, unguessable without the per-profile secret file `<state_dir>/agent-event-secret` (32 random bytes, created once). No supervisor protocol bump is needed.
2. **The env vars are added in `create_pty`** (`app/src/terminal/local_tty/terminal_manager.rs:1805`), which is where the session id and the env map meet; they then reach both `local_tty/unix.rs` and `local_tty/windows/environment.rs` through `PtyOptions::env_vars`. Gated on `FeatureFlag::HOANotifications`, like `WARP_CLI_AGENT_PROTOCOL_VERSION`. WSL forwarding (`WSLENV`) is out of scope.
3. **Remote-control protocol bump 2 → 3** (`app/src/remote_control/protocol.rs:17`) for the new `Request::CliAgentEvent`. The notification travels as a JSON string, not as `CLIAgentNotification`: that type carries `serde_json::Value` and `skip_serializing_none`, neither of which survives bincode.
4. **Injection point.** `TerminalView` already turns `ModelEvent::PluggableNotification { title: Some("warp://cli-agent"), body }` into listener registration (`view.rs:13172`, `13549-13627`) and the listener subscribes to the same dispatcher (`listener/mod.rs:189`). Re-emitting there means no second code path.
5. **Kimi `SessionEnd`** sends nothing: the protocol has no "session ended" event; the pane's agent session ends when `kimi` exits, as for every agent.
6. **"Unless a hook event for the same turn already arrived"** is implemented the way Codex does it: once a session has received any structured event (`received_rich_notification`), Kimi's OSC 9 is dropped.
7. **Icons are named `Icon::KimiLogo` / `Icon::DeepSeekLogo`** (the enum's `*Logo` convention), not `Icon::Kimi` / `Icon::DeepSeek` as in the spec. Both SVGs use the repo's tint convention (`fill="#FF0000"`).
8. **dsh sessions appear in the picker without a Resume action**: `agent_session_actions::action_for` already returns `None` when `resume_command()` is `None`.
9. **The Kimi hooks' version lives in the hook command** (`agent-event --format kimi-hook --hooks-version 1.0.0`), not in a TOML comment as the spec says: each hook then reports it as `plugin_version`, which is what the footer's update check compares (`agent_input_footer/mod.rs:1161-1177`). Without a reported version that check shows "Update" forever.
10. **Kimi hook payloads have no `transcript_path`** and only `SessionStart`/`SessionEnd` carry the session's cwd. `agent-event` looks the session directory up in `<KIMI_CODE_HOME>/session_index.jsonl`, falling back to Kimi's `wd_<slug>_<hash>` naming, and sends `cwd` only on `session_start`.
11. **dsh event names differ from the spec table where the source requires it**: `tools/result` (the observe-only twin of the `tools/post-execute` waterfall), and `session/event` filtered on `approval/asked` / `approval/decided` (those are session-log entries, not Cordis events). The plugin computes the transcript path itself; dsh does not expose it.
12. **dsh runs as `dsh --profile headless …`**, not `dsh headless …` (`bundle/headless/src/startup.ts:38-52`); tests and the real-machine task use that form.
13. **Kimi's OSC 9 titles are mapped** (`approval required` → `PermissionRequest`, `needs your answer` → `QuestionAsked`, `task complete` → `Stop`) instead of the spec's "everything → stop", because Kimi's titles are fixed strings and a "done" notification for an approval prompt would be wrong.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `app/src/terminal/cli_agent.rs` (+`_tests.rs`) | variants, prefixes, npx detection, names, icons, colours, bash mode, Alt+V helper, history-recovery helper | 1, 2 |
| `app/assets/bundled/svg/kimi.svg`, `deepseek.svg` | brand marks | 1 |
| `crates/warp_core/src/ui/icons.rs` | `Icon::KimiLogo`, `Icon::DeepSeekLogo` | 1 |
| `app/src/server/telemetry/events.rs` | `CLIAgentType::{Kimi, DeepSeekHarness}` | 1 |
| exhaustive-match files (`agent_session_handoff.rs`, `process_registry.rs`, `workspace/view.rs`, `plugin_manager/mod.rs`, `agent_session_history.rs`, `app_state.rs`, `use_agent_footer/mod.rs`) | new arms | 1 |
| `app/src/terminal/cli_agent_sessions/listener/mod.rs` (+`mod_tests.rs`) | supported agents, `KimiSessionHandler` | 2, 3 |
| `app/src/terminal/cli_agent_sessions/event/mod.rs` | `CLIAgentEventSource::KimiOsc9Fallback` | 3 |
| `app/src/terminal/view.rs` | Alt+V, OSC 9 suppression, proactive Kimi listener, token storage, out-of-band entry | 2, 3, 7, 8 |
| `crates/input_classifier/src/util.rs` (+tests) | `kimi`, `dsh` keywords | 2 |
| `app/src/ai/agent_management/agent_management_model.rs` | notification messages | 2 |
| `app/src/terminal/cli_agent_sessions/mod.rs` | history-recovery allow-list | 2 |
| `app/src/app_state.rs` (+tests) | Kimi resume command | 4 |
| `app/src/terminal/agent_session_history.rs` (+tests) | `session_id_from_command`, Kimi and dsh scanners | 4, 5, 6 |
| `app/src/terminal/agent_session_paths.rs` (+tests) | Kimi/dsh data roots, Kimi session index, `kimi_work_dir_key` | 5, 9 |
| `app/Cargo.toml` | `zstd` direct dependency | 6 |
| `crates/warp_core/src/cli_agent_protocol.rs` | env var names | 7 |
| `app/src/terminal/agent_event_token.rs` (+tests) | token derivation, secret file, env vars | 7 |
| `app/src/terminal/local_tty/terminal_manager.rs` | env insertion in `create_pty`, token to surface | 7 |
| `app/src/terminal/writeable_pty/terminal_surface.rs` | `on_agent_event_token_determined` | 7 |
| `app/src/terminal/model_events.rs` | `emit_out_of_band_cli_agent_notification` | 8 |
| `app/src/remote_control/{protocol,handler,client}.rs` (+tests) | `CliAgentEvent`, delivery, `call_within`, protocol v3 | 8 |
| `app/src/workspace/view.rs` | `terminal_view_with_agent_event_token` | 8 |
| `crates/warp_cli/src/{agent_event.rs,lib.rs}` (+`lib_tests.rs`) | `agent-event` arguments | 9 |
| `app/src/agent_event_cli/{mod.rs,kimi_hook.rs}` (+tests) | hook mapping, delivery with deadline | 9 |
| `app/src/lib.rs` | module and dispatch | 9 |
| `crates/warp_features/src/lib.rs` | `KimiNotifications`, `DshNotifications` | 10 |
| `app/src/terminal/cli_agent_sessions/plugin_manager/{mod,kimi}.rs` (+tests) | Kimi hook installer | 10 |
| `app/resources/dsh-warp/…` | Cordis plugin `@stozn/dsh-warp` | 11 |
| `app/src/terminal/cli_agent_sessions/plugin_manager/dsh.rs` (+tests) | dsh plugin installer | 12 |
---

### Task 0: Branch check

- [ ] **Step 1: Confirm the worktree and branch**

Run (PowerShell or Git Bash):
```bash
git -C T:/projects/dev-ops/warp-kimi-dsh status --short --branch
git -C T:/projects/dev-ops/warp-kimi-dsh log --oneline -1
```
Expected: `## feat/kimi-dsh-agents...gitlab/feat/kimi-dsh-agents`, clean tree, head `b74fe1fe docs(spec): Kimi Code and DeepSeek Harness agents design` or later. If the branch is missing, stop and ask; do not create it on `master`.

- [ ] **Step 2: Reuse a sibling build cache (optional, saves ~40 min)**

```powershell
New-Item -ItemType Directory -Force T:\projects\dev-ops\warp-kimi-dsh\target\debug | Out-Null
$env:CARGO_TARGET_DIR = "T:\projects\dev-ops\warp\target"
```
Only if `T:\projects\dev-ops\warp\target` exists and no build is running there. Never `git worktree remove --force` this worktree while `target\warpspeed-merge` is a junction.

---

### Task 1: `CLIAgent::Kimi` and `CLIAgent::DeepSeekHarness`

Template: commit `8a30a37f feat: add oh-my-pi as a native CLI agent` (same 12 files). Values that only appear in exhaustive matches are set to their final value here; non-exhaustive lists are Task 2.

**Files:**
- Create: `app/assets/bundled/svg/kimi.svg`, `app/assets/bundled/svg/deepseek.svg`
- Modify: `app/src/terminal/cli_agent.rs:34-160` (colours, enum), `:164-191` (prefixes), `:227-247` (names), `:250-273` (icons), `:278-306` (skills), `:336-356` (brand colours), `:384-391` (`matches_command`), `:611-633` (telemetry)
- Modify: `crates/warp_core/src/ui/icons.rs:286` and `:634`
- Modify: `app/src/server/telemetry/events.rs:466`
- Modify (add both variants to the existing "unsupported" arm): `app/src/terminal/cli_agent_sessions/listener/mod.rs:73-80`, `app/src/terminal/cli_agent_sessions/plugin_manager/mod.rs:288-303`, `app/src/app_state.rs:309-322`, `app/src/terminal/agent_session_history.rs:250-263, 293-306, 473-485`, `app/src/terminal/agent_session_handoff.rs:105-121, 219-232, 243-262, 640-654`, `app/src/terminal/cli_agent_sessions/process_registry.rs:268-283, 349-364`, `app/src/workspace/view.rs:13696-13711`
- Modify: `app/src/terminal/view/use_agent_footer/mod.rs:124-145`
- Test: `app/src/terminal/cli_agent_tests.rs`, `app/src/terminal/view/use_agent_footer/mod_tests.rs`

**Interfaces:**
- Produces: `CLIAgent::Kimi` (serialized `"Kimi"`, prefix `"kimi"`), `CLIAgent::DeepSeekHarness` (serialized `"DeepSeekHarness"`, prefix `"dsh"`), `Icon::KimiLogo`, `Icon::DeepSeekLogo`, `CLIAgentType::Kimi`, `CLIAgentType::DeepSeekHarness`, `const DSH_NPM_PACKAGE: &str = "@deepseek-ai/dsh"` (private to `cli_agent.rs`). `event::v1::resolve_agent("kimi")` → `Kimi`, `resolve_agent("dsh")` → `DeepSeekHarness` (no change needed there: it searches `command_prefixes`).

- [ ] **Step 1: Write the failing tests**

Append to `app/src/terminal/cli_agent_tests.rs`:

```rust
#[test]
fn test_detect_kimi_and_deepseek_harness() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            for (command, expected) in [
                ("kimi", CLIAgent::Kimi),
                ("kimi -c", CLIAgent::Kimi),
                (
                    "kimi --session session_0b6c1234-ffff-4a4a-8b8b-0123456789ab",
                    CLIAgent::Kimi,
                ),
                ("dsh --profile headless \"fix the build\"", CLIAgent::DeepSeekHarness),
                ("npx @deepseek-ai/dsh --profile headless hi", CLIAgent::DeepSeekHarness),
                (
                    "npx -y @deepseek-ai/dsh@0.2.1-alpha.1 --profile headless hi",
                    CLIAgent::DeepSeekHarness,
                ),
                ("npx --yes @deepseek-ai/dsh", CLIAgent::DeepSeekHarness),
                ("FOO=1 npx -y @deepseek-ai/dsh", CLIAgent::DeepSeekHarness),
                (r"C:\nodejs\npx.cmd @deepseek-ai/dsh", CLIAgent::DeepSeekHarness),
            ] {
                assert_eq!(
                    CLIAgent::detect(command, None, None, ctx),
                    Some(expected),
                    "failed to detect {command}",
                );
            }
            for command in [
                "npx",
                "npx prettier --write .",
                "npx -y @deepseek-ai/dsh-other",
                "kimichanga",
            ] {
                assert_eq!(
                    CLIAgent::detect(command, None, None, ctx),
                    None,
                    "{command} is not an agent"
                );
            }
        });
    });
}

#[test]
fn test_kimi_and_deepseek_harness_identity() {
    use ai::skills::SkillProvider;

    assert_eq!(CLIAgent::Kimi.display_name(), "Kimi Code");
    assert_eq!(CLIAgent::DeepSeekHarness.display_name(), "DeepSeek Harness");
    assert_eq!(CLIAgent::Kimi.command_prefix(), "kimi");
    assert_eq!(CLIAgent::DeepSeekHarness.command_prefix(), "dsh");
    assert_eq!(CLIAgent::Kimi.icon(), Some(Icon::KimiLogo));
    assert_eq!(CLIAgent::DeepSeekHarness.icon(), Some(Icon::DeepSeekLogo));
    assert_eq!(
        CLIAgent::Kimi.supported_skill_providers(),
        &[SkillProvider::Agents]
    );
    assert!(CLIAgent::DeepSeekHarness.supported_skill_providers().is_empty());
    assert_eq!(CLIAgent::Kimi.skill_command_prefix(), "/");
    assert_eq!(CLIAgent::DeepSeekHarness.skill_command_prefix(), "/");
    assert_eq!(
        CLIAgent::Kimi.brand_color(),
        Some(ColorU::new(0x17, 0x83, 0xFF, 0xFF))
    );
    assert_eq!(
        CLIAgent::DeepSeekHarness.brand_color(),
        Some(ColorU::new(0x4D, 0x6B, 0xFE, 0xFF))
    );
    assert_eq!(CLIAgent::Kimi.brand_icon_color(), ColorU::white());
    assert_eq!(CLIAgent::DeepSeekHarness.brand_icon_color(), ColorU::white());
    assert!(matches!(
        crate::server::telemetry::CLIAgentType::from(CLIAgent::Kimi),
        crate::server::telemetry::CLIAgentType::Kimi
    ));
    assert!(matches!(
        crate::server::telemetry::CLIAgentType::from(CLIAgent::DeepSeekHarness),
        crate::server::telemetry::CLIAgentType::DeepSeekHarness
    ));
}
```

Append to `app/src/terminal/view/use_agent_footer/mod_tests.rs`:

```rust
#[test]
fn test_rich_input_submit_strategy_for_kimi_and_deepseek_harness() {
    assert_eq!(
        rich_input_submit_strategy(CLIAgent::Kimi),
        RichInputSubmitStrategy::BracketedPaste
    );
    assert_eq!(
        rich_input_submit_strategy(CLIAgent::DeepSeekHarness),
        RichInputSubmitStrategy::Inline
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p warp terminal::cli_agent::tests use_agent_footer::tests::test_rich_input_submit_strategy`
Expected: compile error `no variant named Kimi found for enum CLIAgent`.

- [ ] **Step 3: Add the SVGs**

`app/assets/bundled/svg/kimi.svg` (from `kimi-code/apps/vscode/resources/kimi-icon.svg`, MIT, recoloured to the tint colour):

```svg
<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <rect x="3" y="4.5" width="18" height="13" rx="2.2" fill="none" stroke="#FF0000" stroke-width="1.6"/>
  <rect x="9.6" y="8.0" width="1.4" height="2.6" rx="0.45" fill="#FF0000"/>
  <rect x="15.6" y="8.0" width="1.4" height="2.6" rx="0.45" fill="#FF0000"/>
</svg>
```

`app/assets/bundled/svg/deepseek.svg` (from `deepseek-harness/apps/web/public/favicon.svg`, MIT, recoloured):

```svg
<svg width="24" height="24" viewBox="0 0 50 50" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path fill="#FF0000" fill-rule="nonzero" d="M48.8354 10.0479C48.3232 9.79199 48.1025 10.2798 47.8032 10.5278C47.7007 10.6079 47.6143 10.7119 47.5273 10.8076C46.7793 11.624 45.9048 12.1597 44.7622 12.0957C43.0923 12 41.666 12.5356 40.4058 13.8398C40.1377 12.2319 39.2476 11.272 37.8926 10.6558C37.1836 10.3359 36.4668 10.0156 35.9702 9.31982C35.6235 8.82373 35.5293 8.27197 35.356 7.72754C35.2456 7.3999 35.1353 7.06396 34.7651 7.00781C34.3633 6.94385 34.2056 7.2876 34.0479 7.57568C33.418 8.75195 33.1733 10.0479 33.1973 11.3599C33.2524 14.312 34.4736 16.6641 36.8999 18.3359C37.1758 18.5278 37.2466 18.7197 37.1597 19C36.9946 19.5757 36.7974 20.1357 36.624 20.7119C36.5137 21.0801 36.3486 21.1597 35.9624 21C34.6309 20.4321 33.481 19.5918 32.4644 18.5757C30.7393 16.8721 29.1792 14.9917 27.2334 13.52C26.7764 13.1758 26.3193 12.856 25.8467 12.5518C23.8618 10.584 26.1069 8.96777 26.627 8.77588C27.1704 8.57568 26.8159 7.8877 25.0591 7.896C23.3022 7.90381 21.6953 8.50391 19.647 9.30371C19.3477 9.42383 19.0322 9.51172 18.7095 9.58398C16.8501 9.22363 14.9199 9.14355 12.9033 9.37598C9.10596 9.80762 6.07275 11.6396 3.84326 14.7681C1.16455 18.5278 0.53418 22.7998 1.30664 27.2559C2.11768 31.9521 4.46582 35.8398 8.07373 38.8799C11.8159 42.0322 16.1255 43.5762 21.041 43.2803C24.0269 43.104 27.3516 42.6963 31.1016 39.4561C32.0469 39.936 33.0396 40.1279 34.686 40.272C35.9546 40.3921 37.1758 40.208 38.1211 40.0078C39.6021 39.688 39.4995 38.2881 38.9639 38.0322C34.623 35.9678 35.5762 36.8081 34.71 36.1279C36.9155 33.4639 40.2402 30.6958 41.54 21.728C41.6426 21.0161 41.5557 20.5679 41.54 19.9917C41.5322 19.6396 41.6108 19.5039 42.0049 19.4639C43.0923 19.3359 44.1479 19.0317 45.1167 18.4878C47.9292 16.9199 49.064 14.3438 49.3315 11.2559C49.3711 10.7837 49.3237 10.2959 48.8354 10.0479ZM24.3262 37.8398C20.1196 34.4639 18.0791 33.3521 17.2358 33.3999C16.4482 33.4482 16.5898 34.3682 16.7632 34.9678C16.9443 35.5601 17.1812 35.9683 17.5117 36.4878C17.7402 36.832 17.8979 37.3442 17.2832 37.728C15.9282 38.584 13.5728 37.4399 13.4624 37.3838C10.7207 35.7358 8.42822 33.5601 6.81348 30.584C5.25342 27.7197 4.34766 24.6479 4.19775 21.3677C4.1582 20.5757 4.38672 20.2959 5.15869 20.1519C6.17529 19.96 7.22314 19.9199 8.23926 20.0718C12.5327 20.7119 16.1885 22.6719 19.2529 25.7759C21.002 27.5439 22.3252 29.6558 23.6885 31.7202C25.1377 33.9121 26.6978 36 28.6831 37.7119C29.3843 38.312 29.9434 38.7681 30.479 39.104C28.8643 39.2881 26.1699 39.3281 24.3262 37.8398ZM26.3433 24.6001C26.3433 24.248 26.6191 23.9678 26.9658 23.9678C27.0444 23.9678 27.1152 23.9839 27.1782 24.0078C27.2651 24.04 27.3438 24.0879 27.4067 24.1602C27.5171 24.272 27.5801 24.4321 27.5801 24.6001C27.5801 24.9521 27.3042 25.2319 26.9575 25.2319C26.6108 25.2319 26.3433 24.9521 26.3433 24.6001ZM32.6064 27.8799C32.2046 28.0479 31.8027 28.1919 31.4165 28.208C30.8179 28.2397 30.1641 27.9922 29.8096 27.688C29.2583 27.2158 28.8643 26.9521 28.6987 26.1279C28.6279 25.7759 28.6675 25.2319 28.7305 24.9199C28.8721 24.248 28.7144 23.8159 28.2495 23.4238C27.8716 23.104 27.3911 23.0161 26.8633 23.0161C26.666 23.0161 26.4849 22.9277 26.3511 22.856C26.1304 22.7441 25.9492 22.4639 26.1226 22.1201C26.1777 22.0078 26.4458 21.7358 26.5088 21.688C27.2256 21.272 28.0527 21.4077 28.8169 21.7197C29.5259 22.0161 30.0615 22.5601 30.834 23.3281C31.6216 24.2559 31.7632 24.5117 32.2124 25.208C32.5669 25.752 32.8901 26.312 33.1104 26.9521C33.2446 27.3521 33.0713 27.6802 32.6064 27.8799Z"/>
</svg>
```

- [ ] **Step 4: Icons and telemetry**

`crates/warp_core/src/ui/icons.rs`: after `OhMyPiLogo,` (line 286) add `KimiLogo,` and `DeepSeekLogo,`; after `Icon::OhMyPiLogo => "bundled/svg/oh_my_pi.svg",` (line 634) add:
```rust
            Icon::KimiLogo => "bundled/svg/kimi.svg",
            Icon::DeepSeekLogo => "bundled/svg/deepseek.svg",
```
`app/src/server/telemetry/events.rs`: after `Antigravity,` in `enum CLIAgentType` (line 471) add `Kimi,` and `DeepSeekHarness,`.

- [ ] **Step 5: `cli_agent.rs`**

After `MISTRAL_ORANGE` (line 136) add:
```rust
/// Kimi brand blue (#1783FF)
const KIMI_BLUE: ColorU = ColorU {
    r: 0x17,
    g: 0x83,
    b: 0xFF,
    a: 255,
};

/// DeepSeek brand indigo (#4D6BFE)
const DEEPSEEK_INDIGO: ColorU = ColorU {
    r: 0x4D,
    g: 0x6B,
    b: 0xFE,
    a: 255,
};

/// The npm package `npx` runs DeepSeek Harness from.
const DSH_NPM_PACKAGE: &str = "@deepseek-ai/dsh";
```
In `enum CLIAgent`, after `Antigravity,` add `Kimi,` and `DeepSeekHarness,` (before the `WarpTui` doc comment). Update the enum doc comment's list to end `…, Mistral Vibe, Kimi Code, DeepSeek Harness)`.

Add the arms (each next to the `Antigravity` arm of the same match):
```rust
// command_prefixes
            CLIAgent::Kimi => &["kimi"],
            CLIAgent::DeepSeekHarness => &["dsh"],
// display_name
            CLIAgent::Kimi => "Kimi Code",
            CLIAgent::DeepSeekHarness => "DeepSeek Harness",
// icon
            CLIAgent::Kimi => Some(Icon::KimiLogo),
            CLIAgent::DeepSeekHarness => Some(Icon::DeepSeekLogo),
// supported_skill_providers
            CLIAgent::Kimi => &[SkillProvider::Agents],
            CLIAgent::DeepSeekHarness => &[],
// brand_color
            CLIAgent::Kimi => Some(KIMI_BLUE),
            CLIAgent::DeepSeekHarness => Some(DEEPSEEK_INDIGO),
// From<CLIAgent> for CLIAgentType
            CLIAgent::Kimi => CLIAgentType::Kimi,
            CLIAgent::DeepSeekHarness => CLIAgentType::DeepSeekHarness,
```
Replace `matches_command` (lines 383-391) with:
```rust
    /// Returns whether the command's executable name identifies this CLI agent.
    pub(super) fn matches_command(&self, command: &str, escape_char: Option<EscapeChar>) -> bool {
        let command = command.trim_start();
        if matches!(self, CLIAgent::DeepSeekHarness) && is_npx_launch_of(command, DSH_NPM_PACKAGE)
        {
            return true;
        }
        let Some(first_word) = Self::extract_first_command(command, escape_char) else {
            return false;
        };
        let basename = first_word.rsplit(['/', '\\']).next().unwrap_or(&first_word);
        self.command_prefixes().contains(&basename)
    }
```
and add after `impl CLIAgent { … }` (before `build_review_prompt`):
```rust
/// Whether `command` runs `package` (optionally `package@version`) through `npx`, after any
/// leading `NAME=value` assignments and `npx` flags such as `-y`.
fn is_npx_launch_of(command: &str, package: &str) -> bool {
    let mut words = command.split_whitespace().skip_while(|word| word.contains('='));
    let Some(runner) = words.next() else {
        return false;
    };
    let runner = runner.rsplit(['/', '\\']).next().unwrap_or(runner);
    if !matches!(runner, "npx" | "npx.cmd") {
        return false;
    }
    words.find(|word| !word.starts_with('-')).is_some_and(|word| {
        word == package
            || word
                .strip_prefix(package)
                .is_some_and(|version| version.starts_with('@'))
    })
}
```

- [ ] **Step 6: Footer strategy**

`app/src/terminal/view/use_agent_footer/mod.rs` `rich_input_submit_strategy`: add `CLIAgent::Kimi => RichInputSubmitStrategy::BracketedPaste,` after the `OhMyPi` arm, and add `| CLIAgent::DeepSeekHarness` to the `Inline` group (after `| CLIAgent::Antigravity`).

- [ ] **Step 7: Exhaustive "unsupported" arms**

Add `| CLIAgent::Kimi | CLIAgent::DeepSeekHarness` (in `workspace/view.rs` spelled `| crate::terminal::CLIAgent::Kimi | crate::terminal::CLIAgent::DeepSeekHarness`) after the `Antigravity` line of each of these arms. Later tasks replace some of them:
- `listener/mod.rs` `create_handler` → `None` group (Task 2 moves them)
- `plugin_manager/mod.rs` `plugin_manager_for_with_shell` → `None` group (Tasks 10, 12)
- `app_state.rs` `CliAgentResumeDescriptor::resume_command` → `None` group (Task 4 moves Kimi)
- `agent_session_history.rs` `describe`, `candidates`, `session_id_from_command` → their "nothing" groups (Tasks 4, 5, 6)
- `agent_session_handoff.rs` `agent_ctrl_profile`, `session_file_path` (bail), `agent_session_root`, `agent_exit_command` (plan D owns these)
- `process_registry.rs` `is_agent_process` (`false`), `registry_directory` (`None`)
- `workspace/view.rs` `resolve_agent_resume_command` → `return None` (spec A·2: fork-at-last-turn stays Claude/Pi; Kimi falls back to `resume_command`)

Then `cargo check -p warp` and add any arm the compiler still reports, following the same rule.

- [ ] **Step 8: Run the tests**

Run: `cargo nextest run -p warp terminal::cli_agent::tests use_agent_footer::tests::test_rich_input_submit_strategy`
Expected: all pass, including the existing `test_serialized_name_round_trips_known_agents`.

- [ ] **Step 9: Commit**

```bash
git add app/assets/bundled/svg/kimi.svg app/assets/bundled/svg/deepseek.svg app/src/terminal/cli_agent.rs app/src/terminal/cli_agent_tests.rs crates/warp_core/src/ui/icons.rs app/src/server/telemetry/events.rs app/src/terminal/view/use_agent_footer/mod.rs app/src/terminal/view/use_agent_footer/mod_tests.rs app/src/terminal/cli_agent_sessions/listener/mod.rs app/src/terminal/cli_agent_sessions/plugin_manager/mod.rs app/src/app_state.rs app/src/terminal/agent_session_history.rs app/src/terminal/agent_session_handoff.rs app/src/terminal/cli_agent_sessions/process_registry.rs app/src/workspace/view.rs
git commit -m "feat(cli-agent): add Kimi Code and DeepSeek Harness agent kinds" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Per-agent lists outside exhaustive matches

**Files:**
- Modify: `app/src/terminal/cli_agent_sessions/listener/mod.rs:39-82`
- Modify: `app/src/terminal/cli_agent.rs:317-328` (`supports_bash_mode`), new `uses_alt_v_image_paste`, new `supports_history_recovery`
- Modify: `app/src/terminal/view.rs:16959-16966` (Alt+V)
- Modify: `app/src/terminal/cli_agent_sessions/mod.rs:430-433`
- Modify: `crates/input_classifier/src/util.rs:23`
- Modify: `app/src/ai/agent_management/agent_management_model.rs:160-163`
- Test: `app/src/terminal/cli_agent_tests.rs`, `app/src/terminal/cli_agent_sessions/listener/mod_tests.rs`, `crates/input_classifier/src/util_tests.rs`

**Interfaces:**
- Consumes: Task 1 variants.
- Produces: `CLIAgent::uses_alt_v_image_paste(&self) -> bool`, `CLIAgent::supports_history_recovery(&self) -> bool` (both `pub(crate)`); `is_agent_supported(&CLIAgent::Kimi | &CLIAgent::DeepSeekHarness) == true`; `create_handler(&CLIAgent::DeepSeekHarness)` → `DefaultSessionListener`; `create_handler(&CLIAgent::Kimi)` → `DefaultSessionListener` until Task 3 swaps in `KimiSessionHandler`.

- [ ] **Step 1: Write the failing tests**

Append to `app/src/terminal/cli_agent_tests.rs`:
```rust
#[test]
fn test_kimi_supports_bash_mode_and_deepseek_harness_does_not() {
    assert!(CLIAgent::Kimi.supports_bash_mode());
    assert!(!CLIAgent::DeepSeekHarness.supports_bash_mode());
}

#[test]
fn test_alt_v_image_paste_agents() {
    assert!(CLIAgent::Claude.uses_alt_v_image_paste());
    assert!(CLIAgent::Kimi.uses_alt_v_image_paste());
    assert!(!CLIAgent::DeepSeekHarness.uses_alt_v_image_paste());
    assert!(!CLIAgent::Codex.uses_alt_v_image_paste());
}

#[test]
fn test_history_recovery_agents() {
    for agent in [
        CLIAgent::Claude,
        CLIAgent::Codex,
        CLIAgent::Pi,
        CLIAgent::Kimi,
        CLIAgent::DeepSeekHarness,
    ] {
        assert!(agent.supports_history_recovery(), "{agent:?}");
    }
    assert!(!CLIAgent::Gemini.supports_history_recovery());
}
```
Append to `app/src/terminal/cli_agent_sessions/listener/mod_tests.rs`:
```rust
#[test]
fn kimi_and_deepseek_harness_are_supported() {
    assert!(is_agent_supported(&CLIAgent::Kimi));
    assert!(is_agent_supported(&CLIAgent::DeepSeekHarness));
    assert!(create_handler(&CLIAgent::Kimi).is_some());
    assert!(create_handler(&CLIAgent::DeepSeekHarness).is_some());
}

#[test]
fn deepseek_harness_end_to_end_parsing_and_handling() {
    let mut handler = create_handler(&CLIAgent::DeepSeekHarness).expect("should create handler");
    let start = handler
        .try_parse(
            Some(CLI_AGENT_NOTIFICATION_SENTINEL),
            r#"{"v":1,"agent":"dsh","event":"session_start","session_id":"0b6c1234-ffff-4a4a-8b8b-0123456789ab"}"#,
            false,
        )
        .expect("session_start parses");
    assert_eq!(start.agent, CLIAgent::DeepSeekHarness);
    assert!(handler.handle_event(start).is_none());

    let stop = handler
        .try_parse(
            Some(CLI_AGENT_NOTIFICATION_SENTINEL),
            r#"{"v":1,"agent":"dsh","event":"stop"}"#,
            false,
        )
        .expect("stop parses");
    let handled = handler.handle_event(stop).expect("stop is forwarded");
    assert_eq!(handled.agent, CLIAgent::DeepSeekHarness);
    assert_eq!(handled.event, CLIAgentEventType::Stop);
}
```
Append to `crates/input_classifier/src/util_tests.rs`:
```rust
#[test]
fn kimi_and_dsh_are_one_off_shell_command_keywords() {
    assert!(is_one_off_shell_command_keyword("kimi"));
    assert!(is_one_off_shell_command_keyword("dsh"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp terminal::cli_agent::tests cli_agent_sessions::listener::tests` and `cargo nextest run -p input_classifier`
Expected: compile errors for the two new methods; then assertion failures for the lists.

- [ ] **Step 3: Implement**

`cli_agent.rs` — `supports_bash_mode` becomes:
```rust
        matches!(
            self,
            CLIAgent::Claude
                | CLIAgent::Codex
                | CLIAgent::OpenCode
                | CLIAgent::OhMyPi
                | CLIAgent::Kimi
        )
```
and add after it:
```rust
    /// Whether this CLI agent reads a clipboard image on Alt+V, which Warp sends on Windows when
    /// an image is pasted into the agent.
    pub(crate) fn uses_alt_v_image_paste(&self) -> bool {
        matches!(self, CLIAgent::Claude | CLIAgent::Kimi)
    }

    /// Whether a session id this agent did not report can be recovered from its session history
    /// on disk.
    pub(crate) fn supports_history_recovery(&self) -> bool {
        matches!(
            self,
            CLIAgent::Claude
                | CLIAgent::Codex
                | CLIAgent::Pi
                | CLIAgent::Kimi
                | CLIAgent::DeepSeekHarness
        )
    }
```
`view.rs:16959-16966` becomes:
```rust
                // On Windows, Claude Code and Kimi Code read a clipboard image on Alt+V.
                let uses_alt_v = CLIAgentSessionsModel::as_ref(ctx)
                    .session(self.view_id)
                    .is_some_and(|s| s.agent.uses_alt_v_image_paste());
                if uses_alt_v {
                    self.write_user_bytes_to_pty(vec![escape_sequences::C0::ESC, b'v'], ctx);
                    return;
                }
```
`cli_agent_sessions/mod.rs:431-433` becomes:
```rust
        if !agent.supports_history_recovery() {
            return;
        }
```
`listener/mod.rs`: add `| CLIAgent::Kimi | CLIAgent::DeepSeekHarness` to `is_agent_supported`; in `create_handler` move both out of the `None` arm into the `DefaultSessionListener` arm, and extend its comment: `Kimi Code (hooks) and DeepSeek Harness (Cordis plugin) report through \`warp agent-event\`, which re-emits the same structured events.`
`input_classifier/src/util.rs:23`: add `"kimi", "dsh"` after `"omp"`; extend the doc sentence to name Kimi Code and DeepSeek Harness.
`agent_management_model.rs:160-163`:
```rust
                    let message = match agent {
                        CLIAgent::Codex => "Notification from Codex",
                        CLIAgent::Kimi => "Notification from Kimi Code",
                        CLIAgent::DeepSeekHarness => "Notification from DeepSeek Harness",
                        CLIAgent::Claude
                        | CLIAgent::Pi
                        | CLIAgent::Gemini
                        | CLIAgent::Amp
                        | CLIAgent::Droid
                        | CLIAgent::OpenCode
                        | CLIAgent::Copilot
                        | CLIAgent::OhMyPi
                        | CLIAgent::Auggie
                        | CLIAgent::CursorCli
                        | CLIAgent::Goose
                        | CLIAgent::Hermes
                        | CLIAgent::Vibe
                        | CLIAgent::Antigravity
                        | CLIAgent::WarpTui
                        | CLIAgent::Unknown => "Task completed.",
                    };
```
(The `_` arm is replaced by the remaining variants, as Global Constraints require. If the compiler reports a variant this list misses or repeats, adjust the list.)

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp terminal::cli_agent::tests cli_agent_sessions::listener::tests agent_management` and `cargo nextest run -p input_classifier`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add app/src/terminal/cli_agent.rs app/src/terminal/cli_agent_tests.rs app/src/terminal/view.rs app/src/terminal/cli_agent_sessions/mod.rs app/src/terminal/cli_agent_sessions/listener/mod.rs app/src/terminal/cli_agent_sessions/listener/mod_tests.rs crates/input_classifier/src/util.rs crates/input_classifier/src/util_tests.rs app/src/ai/agent_management/agent_management_model.rs
git commit -m "feat(cli-agent): list Kimi Code and DeepSeek Harness as supported agents" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `KimiSessionHandler` — hook events plus Kimi's OSC 9 fallback

Kimi's TUI writes `ESC ] 9 ; <title>: <body> BEL` under `TERM_PROGRAM=WarpTerminal` (`kimi-code/apps/kimi-code/src/tui/utils/terminal-notification.ts:74-111`; body collapsed to one line, cut to 240 chars; by default only while the terminal is unfocused). Titles: `Kimi Code task complete` (body = session title), `Kimi Code approval required` (body = tool name), `Kimi Code needs your answer` (body = first question). The handler maps those three to `Stop` / `PermissionRequest` / `QuestionAsked` — a refinement of the spec's "→ stop", possible because Kimi's titles are fixed strings — and drops all OSC 9 once a structured event has arrived (Decision 6).

**Files:**
- Modify: `app/src/terminal/cli_agent_sessions/event/mod.rs:27-34`
- Modify: `app/src/terminal/cli_agent_sessions/listener/mod.rs:54-162`
- Modify: `app/src/terminal/view.rs:13182-13190` (OSC 9 suppression), `:12418-12426` (proactive listener), `:13634-13643` (plugin version)
- Test: `app/src/terminal/cli_agent_sessions/listener/mod_tests.rs`

**Interfaces:**
- Consumes: Task 2 (`create_handler(&CLIAgent::Kimi)` exists).
- Produces: `CLIAgentEventSource::KimiOsc9Fallback`; `KimiSessionHandler` (private) with `fn parse_osc9_text(body: &str) -> Option<CLIAgentEvent>`; `create_handler(&CLIAgent::Kimi)` → `KimiSessionHandler`, whose `handle_event` forwards every event including `SessionStart` (its listener usually exists before the first hook event, so `SessionStart` is not consumed by registration).

- [ ] **Step 1: Write the failing tests**

Append to `listener/mod_tests.rs`:
```rust
#[test]
fn kimi_osc9_titles_map_to_their_events() {
    let done = KimiSessionHandler::parse_osc9_text("Kimi Code task complete: Fix the reboot bug")
        .expect("task complete parses");
    assert_eq!(done.agent, CLIAgent::Kimi);
    assert_eq!(done.event, CLIAgentEventType::Stop);
    assert_eq!(done.payload.query.as_deref(), Some("Fix the reboot bug"));
    assert_eq!(done.source, CLIAgentEventSource::KimiOsc9Fallback);

    let approval = KimiSessionHandler::parse_osc9_text("Kimi Code approval required: Shell")
        .expect("approval parses");
    assert_eq!(approval.event, CLIAgentEventType::PermissionRequest);
    assert_eq!(approval.payload.tool_name.as_deref(), Some("Shell"));
    assert_eq!(approval.payload.summary.as_deref(), Some("Shell"));

    let question =
        KimiSessionHandler::parse_osc9_text("Kimi Code needs your answer: Which branch?")
            .expect("question parses");
    assert_eq!(question.event, CLIAgentEventType::QuestionAsked);
    assert_eq!(question.payload.summary.as_deref(), Some("Which branch?"));

    let untitled = KimiSessionHandler::parse_osc9_text("Kimi Code task complete")
        .expect("a title alone parses");
    assert_eq!(untitled.event, CLIAgentEventType::Stop);
    assert_eq!(
        untitled.payload.query.as_deref(),
        Some("Kimi Code task complete")
    );
    assert!(KimiSessionHandler::parse_osc9_text("   ").is_none());
}

#[test]
fn kimi_handler_takes_hook_events_and_forwards_session_start() {
    let mut handler = create_handler(&CLIAgent::Kimi).expect("should create handler");
    let start = handler
        .try_parse(
            Some(CLI_AGENT_NOTIFICATION_SENTINEL),
            r#"{"v":1,"agent":"kimi","event":"session_start","session_id":"session_0b6c1234-ffff-4a4a-8b8b-0123456789ab","plugin_version":"1.0.0"}"#,
            false,
        )
        .expect("session_start parses");
    let forwarded = handler.handle_event(start).expect("session_start is forwarded");
    assert_eq!(forwarded.event, CLIAgentEventType::SessionStart);
    assert_eq!(
        forwarded.session_id.as_deref(),
        Some("session_0b6c1234-ffff-4a4a-8b8b-0123456789ab")
    );
}

#[test]
fn kimi_handler_ignores_other_agents_and_osc9_once_hooks_report() {
    let mut handler = create_handler(&CLIAgent::Kimi).expect("should create handler");
    assert!(
        handler
            .try_parse(
                Some(CLI_AGENT_NOTIFICATION_SENTINEL),
                r#"{"v":1,"agent":"claude","event":"stop"}"#,
                false,
            )
            .is_none()
    );
    assert!(
        handler
            .try_parse(Some("some-title"), "Kimi Code task complete: x", false)
            .is_none()
    );
    assert!(
        handler
            .try_parse(None, "Kimi Code task complete: x", false)
            .is_some()
    );
    assert!(
        handler
            .try_parse(None, "Kimi Code task complete: x", true)
            .is_none()
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp cli_agent_sessions::listener::tests`
Expected: compile error (`KimiSessionHandler` / `KimiOsc9Fallback` not found).

- [ ] **Step 3: Implement**

`event/mod.rs` `enum CLIAgentEventSource`, after `CodexOsc9Fallback`:
```rust
    /// Kimi Code's own OSC 9 notification, used until its hooks report.
    KimiOsc9Fallback,
```
`listener/mod.rs`: in `create_handler` move `CLIAgent::Kimi` out of the `DefaultSessionListener` arm into its own `CLIAgent::Kimi => Some(Box::new(KimiSessionHandler)),`; then add after `impl CLIAgentSessionHandler for CodexSessionHandler`:
```rust
/// Kimi Code reports through its hooks, which `warp agent-event` delivers as structured events.
/// Its TUI also sends OSC 9 on turn end, approval and question, with fixed titles, which stands
/// in for the hooks until they report.
struct KimiSessionHandler;

impl KimiSessionHandler {
    const TASK_COMPLETE: &'static str = "Kimi Code task complete";
    const APPROVAL_REQUIRED: &'static str = "Kimi Code approval required";
    const NEEDS_ANSWER: &'static str = "Kimi Code needs your answer";

    /// Parses a Kimi OSC 9 body (`<title>: <detail>`). Returns `None` only for empty bodies.
    fn parse_osc9_text(body: &str) -> Option<CLIAgentEvent> {
        let body = body.trim();
        if body.is_empty() {
            return None;
        }
        let (title, detail) = body
            .split_once(": ")
            .map(|(title, detail)| (title, Some(detail.trim().to_owned())))
            .unwrap_or((body, None));
        let (event, payload) = match title {
            Self::APPROVAL_REQUIRED => (
                CLIAgentEventType::PermissionRequest,
                CLIAgentEventPayload {
                    summary: detail.clone(),
                    tool_name: detail,
                    ..Default::default()
                },
            ),
            Self::NEEDS_ANSWER => (
                CLIAgentEventType::QuestionAsked,
                CLIAgentEventPayload {
                    summary: detail,
                    ..Default::default()
                },
            ),
            Self::TASK_COMPLETE => (
                CLIAgentEventType::Stop,
                CLIAgentEventPayload {
                    query: detail.or_else(|| Some(body.to_owned())),
                    ..Default::default()
                },
            ),
            _ => (
                CLIAgentEventType::Stop,
                CLIAgentEventPayload {
                    query: Some(body.to_owned()),
                    ..Default::default()
                },
            ),
        };
        Some(CLIAgentEvent {
            v: 1,
            agent: CLIAgent::Kimi,
            event,
            session_id: None,
            cwd: None,
            project: None,
            payload,
            source: CLIAgentEventSource::KimiOsc9Fallback,
        })
    }
}

impl CLIAgentSessionHandler for KimiSessionHandler {
    fn try_parse(
        &mut self,
        title: Option<&str>,
        body: &str,
        plugin_already_active: bool,
    ) -> Option<CLIAgentEvent> {
        if let Some(event) = parse_event(title, body) {
            return (event.agent == CLIAgent::Kimi).then_some(event);
        }
        if title.is_some() || plugin_already_active {
            return None;
        }
        Self::parse_osc9_text(body)
    }

    fn handle_event(&mut self, event: CLIAgentEvent) -> Option<CLIAgentEvent> {
        Some(event)
    }
}
```
(The `match title` is on `&str`; its `_` arm is a string fallback, not an enum wildcard.)

`view.rs:13182-13190` (OSC 9 suppression) becomes:
```rust
                // Suppress OSC 9 notifications when a Codex or Kimi listener is active.
                // The listener's subscription handles these via its session handler.
                if title.is_none() {
                    let has_osc9_listener = CLIAgentSessionsModel::as_ref(ctx)
                        .session(self.view_id)
                        .is_some_and(|s| {
                            matches!(s.agent, CLIAgent::Codex | CLIAgent::Kimi)
                                && s.listener.is_some()
                        });
                    if has_osc9_listener {
                        return;
                    }
                }
```
`view.rs:12418-12426` becomes:
```rust
                                    // Codex and Kimi Code have native OSC 9 notifications, so
                                    // create the listener proactively on command detection
                                    // (rather than waiting for a SessionStart event).
                                    if let Some((agent @ (CLIAgent::Codex | CLIAgent::Kimi), _)) =
                                        &detection
                                    {
                                        me.register_cli_agent_listener_without_session_start_event(
                                            *agent, ctx,
                                        );
                                    }
```
`view.rs:13634-13643`: change `if matches!(agent, CLIAgent::Codex) {` to `if matches!(agent, CLIAgent::Codex | CLIAgent::Kimi) {` and its comment to `// No plugin version distinguishes the OSC 9 fallback of Codex and Kimi from their hooks.`

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp cli_agent_sessions::listener::tests` then `cargo check -p warp`
Expected: PASS; builds.

- [ ] **Step 5: Commit**

```bash
git add app/src/terminal/cli_agent_sessions/event/mod.rs app/src/terminal/cli_agent_sessions/listener/mod.rs app/src/terminal/cli_agent_sessions/listener/mod_tests.rs app/src/terminal/view.rs
git commit -m "feat(cli-agent): track Kimi Code sessions from hooks with an OSC 9 fallback" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Resume command and session ids named on the command line

Kimi flags (`kimi-code/apps/kimi-code/src/cli/commands.ts:42-54`): `-S, --session [id]`, hidden alias `-r, --resume [id]`, `-c, --continue`. dsh headless (`deepseek-harness/packages/bundle/headless/src/startup.ts:38-52`): `dsh --profile headless [--json] [--session-id <id>] [task...]`.

**Files:**
- Modify: `app/src/app_state.rs:301-325`
- Modify: `app/src/terminal/agent_session_history.rs:452-487`
- Test: `app/src/app_state_tests.rs`, `app/src/terminal/agent_session_history_tests.rs`

**Interfaces:**
- Produces: `CliAgentResumeDescriptor { agent: Kimi, session_id: Some(id), .. }.resume_command() == Some("kimi --session <id>")`; DeepSeekHarness → `None`. `session_id_from_command(CLIAgent::Kimi, …)` reads `-S`, `--session`, `-r`, `--resume`, `--session=<id>`, `--resume=<id>`; `session_id_from_command(CLIAgent::DeepSeekHarness, …)` reads `--session-id`.

- [ ] **Step 1: Write the failing tests**

In `app_state_tests.rs` `agents_with_verified_syntax_generate_a_resume_command`, add to the array:
```rust
        (CLIAgent::Kimi, "kimi --session session-1"),
```
and append:
```rust
/// DeepSeek Harness ships no interactive terminal UI to reopen a session in.
#[test]
fn deepseek_harness_has_no_resume_command() {
    assert_eq!(
        descriptor(CLIAgent::DeepSeekHarness, Some("session-1")).resume_command(),
        None
    );
}
```
Append to `agent_session_history_tests.rs`:
```rust
#[test]
fn kimi_and_dsh_commands_name_their_session() {
    let id = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";
    for (command, expected) in [
        (format!("kimi --session {id}"), Some(id)),
        (format!("kimi -S {id}"), Some(id)),
        (format!("kimi -r {id}"), Some(id)),
        (format!("kimi --resume {id} --yolo"), Some(id)),
        (format!("kimi --session={id}"), Some(id)),
        ("kimi -c".to_owned(), None),
        ("kimi --session".to_owned(), None),
        ("kimi --session --yolo".to_owned(), None),
        ("kimi".to_owned(), None),
    ] {
        assert_eq!(
            super::session_id_from_command(CLIAgent::Kimi, &command).as_deref(),
            expected,
            "{command}"
        );
    }
    assert_eq!(
        super::session_id_from_command(
            CLIAgent::DeepSeekHarness,
            "dsh --profile headless --session-id session-0b6c1234-ffff-4a4a-8b8b-0123456789ab \"go on\"",
        )
        .as_deref(),
        Some("session-0b6c1234-ffff-4a4a-8b8b-0123456789ab")
    );
    assert_eq!(
        super::session_id_from_command(
            CLIAgent::DeepSeekHarness,
            "dsh --profile headless \"hi\""
        ),
        None
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp app_state::tests agent_session_history::tests::kimi_and_dsh`
Expected: FAIL (`None` where an id is expected).

- [ ] **Step 3: Implement**

`app_state.rs` `resume_command`: add `CLIAgent::Kimi => Some(format!("kimi --session {session_id}")),` after the `Pi` arm and remove `| CLIAgent::Kimi` from the `None` group; above the `DeepSeekHarness` entry of the `None` group add the comment `// DeepSeek Harness has no interactive surface to resume into.`

`agent_session_history.rs` `session_id_from_command`: add, before the `CLIAgent::Gemini` group, and remove both variants from that group:
```rust
        CLIAgent::Kimi => ["-S", "--session", "-r", "--resume"]
            .into_iter()
            .find_map(argument_after)
            .or_else(|| {
                words.iter().find_map(|word| {
                    word.strip_prefix("--session=")
                        .or_else(|| word.strip_prefix("--resume="))
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                })
            }),
        CLIAgent::DeepSeekHarness => argument_after("--session-id"),
```

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp app_state::tests agent_session_history::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add app/src/app_state.rs app/src/app_state_tests.rs app/src/terminal/agent_session_history.rs app/src/terminal/agent_session_history_tests.rs
git commit -m "feat(cli-agent): resume Kimi Code sessions and read session ids from kimi and dsh commands" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Kimi session history (picker and recovery)

Kimi on disk (`kimi-code/packages/agent-core-v2/src/…`): data root `$KIMI_CODE_HOME` (empty = unset) else `~/.kimi-code`; `session_index.jsonl` at the root, one `{"sessionId","sessionDir","workDir"}` line per created session (`sessionDir` absolute, OS separators), tombstones `{"sessionId","deleted":true}`; `<sessionDir>/state.json` with `id`, `version: 2`, `cwd`, `createdAt`/`updatedAt` as **epoch milliseconds** (legacy ISO strings still accepted), `archived`, optional `title`, `lastPrompt`; legacy `workDir` read as `cwd`.

**Files:**
- Create: `app/src/terminal/agent_session_paths.rs`, `app/src/terminal/agent_session_paths_tests.rs`
- Modify: `app/src/terminal/mod.rs` (module declaration)
- Modify: `app/src/terminal/agent_session_history.rs:1-14` (imports), `:388-438` (`scan`, agent list), new Kimi functions
- Test: `app/src/terminal/agent_session_history_tests.rs`

**Interfaces:**
- Produces (`crate::terminal::agent_session_paths`, all `pub(crate)`):
  - `fn kimi_home() -> Option<PathBuf>`
  - `fn kimi_indexed_session_dirs(home: &Path) -> Option<Vec<(String, PathBuf)>>` — `None` when there is no index; deleted ids omitted; the last line for an id wins
- Produces (`agent_session_history`, private, used in tests): `fn kimi_sessions(home: &Path) -> Vec<PastAgentSession>`; `fn directory_sessions(agent: CLIAgent) -> Option<Vec<PastAgentSession>>`
- `RESUMABLE_AGENTS` becomes `[CLIAgent; 5]` = Claude, Codex, Pi, Kimi, DeepSeekHarness (dsh rows show without a Resume action, Decision 8).

- [ ] **Step 1: Write the failing tests**

`app/src/terminal/agent_session_paths_tests.rs`:
```rust
use std::fs;

use super::kimi_indexed_session_dirs;

#[test]
fn kimi_index_last_line_wins_and_tombstones_remove() {
    let home = tempfile::tempdir().unwrap();
    let kept = home.path().join("sessions").join("wd_proj_4045ff9b01fe").join("session_a");
    let moved = home.path().join("sessions").join("wd_other_123dafdbc83f").join("session_b");
    let lines = [
        serde_json::json!({"sessionId": "session_a", "sessionDir": kept, "workDir": "C:/Users/me/proj"}),
        serde_json::json!({"sessionId": "session_b", "sessionDir": "/old/place", "workDir": "/old"}),
        serde_json::json!({"sessionId": "session_b", "sessionDir": moved, "workDir": "/home/me/other"}),
        serde_json::json!({"sessionId": "session_c", "sessionDir": "/gone", "workDir": "/gone"}),
        serde_json::json!({"sessionId": "session_c", "deleted": true}),
        serde_json::json!({"sessionId": "../escape", "sessionDir": "/x", "workDir": "/x"}),
    ];
    let index = lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>()
        + "not json\n";
    fs::write(home.path().join("session_index.jsonl"), index).unwrap();

    let mut dirs = kimi_indexed_session_dirs(home.path()).expect("the index exists");
    dirs.sort();
    assert_eq!(
        dirs,
        vec![
            ("session_a".to_owned(), kept),
            ("session_b".to_owned(), moved),
        ]
    );
}

#[test]
fn kimi_index_missing_is_none() {
    let home = tempfile::tempdir().unwrap();
    assert!(kimi_indexed_session_dirs(home.path()).is_none());
}
```
Append to `agent_session_history_tests.rs` (add `kimi_sessions` to the `use super::{…}` list and `use std::time::{Duration, SystemTime};` is already imported):
```rust
fn write_file(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

const KIMI_ID: &str = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";

#[test]
fn kimi_index_supplies_the_directory_and_state_the_details() {
    let home = tempfile::tempdir().unwrap();
    let dir = home
        .path()
        .join("sessions")
        .join("wd_proj_4045ff9b01fe")
        .join(KIMI_ID);
    write_file(
        &dir.join("state.json"),
        &format!(
            r#"{{"id":"{KIMI_ID}","version":2,"cwd":"C:/Users/me/proj","createdAt":1788240000000,"updatedAt":1788240666255,"archived":false,"agents":{{}},"custom":{{}},"title":"Fix the reboot bug","titleKind":"generated","lastPrompt":"and the tests too","isCustomTitle":false}}"#
        ),
    );
    let line = serde_json::json!({"sessionId": KIMI_ID, "sessionDir": dir, "workDir": "C:/Users/me/proj"});
    write_file(&home.path().join("session_index.jsonl"), &format!("{line}\n"));

    let sessions = kimi_sessions(home.path());

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.agent, CLIAgent::Kimi);
    assert_eq!(session.session_id, KIMI_ID);
    assert_eq!(session.cwd, Path::new("C:/Users/me/proj"));
    assert_eq!(session.title.as_deref(), Some("Fix the reboot bug"));
    assert_eq!(
        session.modified,
        SystemTime::UNIX_EPOCH + Duration::from_millis(1_788_240_666_255)
    );
}

#[test]
fn kimi_sessions_are_found_without_an_index() {
    let home = tempfile::tempdir().unwrap();
    write_file(
        &home
            .path()
            .join("sessions")
            .join("wd_my-project_123dafdbc83f")
            .join(KIMI_ID)
            .join("state.json"),
        &format!(
            r#"{{"id":"{KIMI_ID}","version":2,"workDir":"/home/me/my project","createdAt":"2026-09-01T09:00:00.000Z","updatedAt":"2026-09-01T10:00:00.000Z","archived":false,"lastPrompt":"Write the release notes"}}"#
        ),
    );

    let sessions = kimi_sessions(home.path());

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].cwd, Path::new("/home/me/my project"));
    assert_eq!(
        sessions[0].title.as_deref(),
        Some("Write the release notes")
    );
    assert_eq!(
        sessions[0].modified,
        SystemTime::UNIX_EPOCH + Duration::from_millis(1_788_256_800_000)
    );
}

#[test]
fn kimi_archived_and_unreadable_sessions_are_skipped() {
    let home = tempfile::tempdir().unwrap();
    let workspace = home.path().join("sessions").join("wd_proj_4045ff9b01fe");
    write_file(
        &workspace.join("session_archived").join("state.json"),
        r#"{"id":"session_archived","version":2,"cwd":"/p","updatedAt":1,"archived":true}"#,
    );
    write_file(&workspace.join("session_broken").join("state.json"), "{");
    fs::create_dir_all(workspace.join("session_empty")).unwrap();

    assert!(kimi_sessions(home.path()).is_empty());
}
```
(`1_788_256_800_000` ms = 2026-09-01T10:00:00Z.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp agent_session_paths agent_session_history::tests::kimi`
Expected: compile error.

- [ ] **Step 3: Implement**

`app/src/terminal/agent_session_paths.rs`:
```rust
//! Where Kimi Code and DeepSeek Harness keep their data, as their own code resolves it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::agent_session_history::is_safe_session_id;

/// Kimi Code's data root: `$KIMI_CODE_HOME`, else `~/.kimi-code`. Kimi's CLI treats an empty
/// value as unset.
pub(crate) fn kimi_home() -> Option<PathBuf> {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".kimi-code")))
}

/// Each live session's directory from Kimi's `session_index.jsonl`, or `None` without an index.
/// Kimi appends a line per created session and a `deleted` tombstone per deletion, so the last
/// line for an id wins.
pub(crate) fn kimi_indexed_session_dirs(home: &Path) -> Option<Vec<(String, PathBuf)>> {
    let file = File::open(home.join("session_index.jsonl")).ok()?;
    let mut dirs: HashMap<String, Option<PathBuf>> = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(session_id) = entry["sessionId"]
            .as_str()
            .filter(|id| is_safe_session_id(id))
        else {
            continue;
        };
        let dir = if entry["deleted"] == true {
            None
        } else {
            let Some(dir) = entry["sessionDir"].as_str() else {
                continue;
            };
            Some(PathBuf::from(dir))
        };
        dirs.insert(session_id.to_owned(), dir);
    }
    Some(
        dirs.into_iter()
            .filter_map(|(session_id, dir)| Some((session_id, dir?)))
            .collect(),
    )
}

#[cfg(test)]
#[path = "agent_session_paths_tests.rs"]
mod tests;
```
`app/src/terminal/mod.rs`, after `pub(crate) mod agent_session_history_model;`: `pub(crate) mod agent_session_paths;`

`agent_session_history.rs`: add `#[cfg(not(target_family = "wasm"))] use super::agent_session_paths::{kimi_home, kimi_indexed_session_dirs};` (Task 6 adds `dsh_home` to it). Add after `vibe_sessions` the following, each function marked `#[cfg(not(target_family = "wasm"))]` (they are reached only through the non-wasm `directory_sessions`; the wasm build would otherwise warn about dead code):
```rust
/// Kimi Code's sessions: directories from its index, or every `sessions/*/*` directory when the
/// index is gone, each described by its `state.json`.
fn kimi_sessions(home: &Path) -> Vec<PastAgentSession> {
    let dirs = kimi_indexed_session_dirs(home).unwrap_or_else(|| {
        directories(&home.join("sessions"))
            .into_iter()
            .flat_map(|workspace| directories(&workspace))
            .filter_map(|dir| {
                let session_id = dir.file_name()?.to_str()?.to_owned();
                is_safe_session_id(&session_id).then_some((session_id, dir))
            })
            .collect()
    });
    dirs.into_iter()
        .filter_map(|(session_id, dir)| kimi_session(session_id, &dir))
        .collect()
}

fn kimi_session(session_id: String, dir: &Path) -> Option<PastAgentSession> {
    let state_path = dir.join("state.json");
    let state: Value = serde_json::from_reader(BufReader::new(File::open(&state_path).ok()?)).ok()?;
    if state["archived"] == true {
        return None;
    }
    let cwd = state["cwd"]
        .as_str()
        .or_else(|| state["workDir"].as_str())
        .map(PathBuf::from)
        .unwrap_or_default();
    let modified = epoch_millis_or_rfc3339(&state["updatedAt"])
        .or_else(|| std::fs::metadata(&state_path).ok()?.modified().ok())?;
    let title = state["title"]
        .as_str()
        .or_else(|| state["lastPrompt"].as_str())
        .and_then(title);
    Some(PastAgentSession {
        agent: CLIAgent::Kimi,
        session_id,
        cwd,
        canonical_cwd: None,
        title,
        modified,
    })
}

/// A timestamp Kimi wrote as epoch milliseconds, or as an RFC 3339 string in older versions.
fn epoch_millis_or_rfc3339(value: &Value) -> Option<SystemTime> {
    let millis = match value.as_u64() {
        Some(millis) => millis,
        None => u64::try_from(
            chrono::DateTime::parse_from_rfc3339(value.as_str()?)
                .ok()?
                .timestamp_millis(),
        )
        .ok()?,
    };
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(millis))
}
```
Replace the Vibe special case in `scan` (lines 389-408) with:
```rust
fn scan(agent: CLIAgent, not_before: Option<SystemTime>) -> Vec<PastAgentSession> {
    #[cfg(not(target_family = "wasm"))]
    let listed = directory_sessions(agent);
    #[cfg(target_family = "wasm")]
    let listed = None;
    let mut sessions = listed.unwrap_or_else(|| {
        let mut candidates = candidates(agent)
            .into_iter()
            .filter(|candidate| not_before.is_none_or(|time| candidate.modified >= time))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| right.modified.cmp(&left.modified));
        candidates.truncate(MAX_CANDIDATES);
        candidates.into_iter().map(describe).collect()
    });
```
(the rest of `scan` is unchanged) and add above `scan`:
```rust
/// The sessions of agents that keep an index or a directory per session, read whole instead of
/// through [`candidates`] and [`describe`]. `None` for every other agent.
#[cfg(not(target_family = "wasm"))]
fn directory_sessions(agent: CLIAgent) -> Option<Vec<PastAgentSession>> {
    match agent {
        CLIAgent::Vibe => Some(
            dirs::home_dir()
                .map(|home| vibe_sessions(&home.join(".vibe").join("logs").join("session")))
                .unwrap_or_default(),
        ),
        CLIAgent::Kimi => Some(kimi_home().map(|home| kimi_sessions(&home)).unwrap_or_default()),
        CLIAgent::Claude
        | CLIAgent::Gemini
        | CLIAgent::Codex
        | CLIAgent::Amp
        | CLIAgent::Droid
        | CLIAgent::OpenCode
        | CLIAgent::Copilot
        | CLIAgent::Pi
        | CLIAgent::OhMyPi
        | CLIAgent::Auggie
        | CLIAgent::CursorCli
        | CLIAgent::Goose
        | CLIAgent::Hermes
        | CLIAgent::Antigravity
        | CLIAgent::DeepSeekHarness
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => None,
    }
}
```
Replace `RESUMABLE_AGENTS` with:
```rust
/// The agents whose past sessions the session picker lists. An agent without a resume command
/// is listed without a Resume action.
pub(crate) const RESUMABLE_AGENTS: [CLIAgent; 5] = [
    CLIAgent::Claude,
    CLIAgent::Codex,
    CLIAgent::Pi,
    CLIAgent::Kimi,
    CLIAgent::DeepSeekHarness,
];
```
Make `is_safe_session_id` stay `pub(crate)` (it already is).

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp agent_session_paths agent_session_history`
Expected: PASS, including the existing Vibe and Pi tests.

- [ ] **Step 5: Commit**

```bash
git add app/src/terminal/agent_session_paths.rs app/src/terminal/agent_session_paths_tests.rs app/src/terminal/mod.rs app/src/terminal/agent_session_history.rs app/src/terminal/agent_session_history_tests.rs
git commit -m "feat(cli-agent): list and recover Kimi Code sessions from its session index" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: DeepSeek Harness session history (zstd)

dsh on disk (`deepseek-harness/packages/session/session-persistence-jsonl/src/{format.ts,index.ts}`): `<DSH_HOME>/sessions/--<projectKey(cwd)>--/<encodeSegment(id)>/session[.vN].jsonl[.zstd]` (`generationLogFilename(0,'none') == "session.jsonl"`, `(27,'zstd') == "session.v27.jsonl.zstd"`; current format version 4); zstd files are **multi-frame** (header one frame, each appended batch another); line 1 is `{"type":"session","version":4,"id","createdAt"(ms),"cwd"?,"isSeeded","delegationDepth",…}`; the first user message is a `{"type":"user/message","data":{"content":[{"type":"text","text":…}],"source":{"kind":"user",…},…}}` row (runtime-context injections are also `user/message` rows, with `source.kind: "runtime-context"`); sessions without a cwd live under `_no-cwd` (skipped here). Headless ids are `session-<uuid>`, ACP ids bare UUIDs.

**Files:**
- Modify: `app/Cargo.toml:354` (add `zstd = "0.13.3"` after `warp_files.workspace = true` in the non-wasm section)
- Modify: `app/src/terminal/agent_session_paths.rs` (`dsh_home`, added here because nothing calls it before this task)
- Modify: `app/src/terminal/agent_session_history.rs` (dsh functions, `directory_sessions` arm)
- Test: `app/src/terminal/agent_session_history_tests.rs`

**Interfaces:**
- Consumes: Task 5 `directory_sessions`, `agent_session_paths` (the module Task 5 creates).
- Produces: `agent_session_paths::dsh_home() -> Option<PathBuf>` (`pub(crate)`); `agent_session_history::current_dsh_log(dir: &Path) -> Option<(PathBuf, bool)>` (`pub(crate)`; path and is-zstd; Plan D's handoff code locates a dsh session file with it, so the generation rule has one parser); private `fn dsh_sessions(root: &Path) -> Vec<PastAgentSession>` (`root` = `<DSH_HOME>/sessions`).

- [ ] **Step 1: Write the failing tests**

Append to `agent_session_history_tests.rs` (add `current_dsh_log, dsh_sessions` to the `use super::{…}` list):
```rust
const DSH_ID: &str = "session-0b6c1234-ffff-4a4a-8b8b-0123456789ab";

fn dsh_header(id: &str) -> String {
    format!(
        r#"{{"type":"session","version":4,"id":"{id}","createdAt":1788240666255,"cwd":"C:\\Users\\me\\proj","isSeeded":false,"delegationDepth":0,"agentPreset":"standard"}}"#
    )
}

const DSH_RUNTIME_CONTEXT: &str = r#"{"type":"user/message","data":{"content":[{"type":"text","text":"<environment>"}],"source":{"kind":"runtime-context"},"role":"user","id":"m0"},"surfaceOp":"append","seq":1,"time":1788240666300}"#;
const DSH_USER: &str = r#"{"type":"user/message","data":{"content":[{"type":"text","text":"Fix the reboot bug"}],"source":{"kind":"user","rpcId":"r1","clientTimeZone":"Europe/Vienna"},"role":"user","id":"m1"},"surfaceOp":"append","seq":2,"time":1788240666400}"#;

#[test]
fn dsh_compressed_log_supplies_identity_directory_and_title() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("--C-Users-me-proj--").join(DSH_ID);
    fs::create_dir_all(&dir).unwrap();
    // One frame for the header and one for the appended batch, as dsh writes them.
    let mut log = zstd::encode_all(format!("{}\n", dsh_header(DSH_ID)).as_bytes(), 3).unwrap();
    log.extend(
        zstd::encode_all(format!("{DSH_RUNTIME_CONTEXT}\n{DSH_USER}\n").as_bytes(), 3).unwrap(),
    );
    fs::write(dir.join("session.v4.jsonl.zstd"), log).unwrap();
    fs::write(
        dir.join("session.v3.jsonl.zstd"),
        zstd::encode_all(format!("{}\n", dsh_header("stale")).as_bytes(), 3).unwrap(),
    )
    .unwrap();

    let sessions = dsh_sessions(root.path());

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.agent, CLIAgent::DeepSeekHarness);
    assert_eq!(session.session_id, DSH_ID);
    assert_eq!(session.cwd, Path::new(r"C:\Users\me\proj"));
    assert_eq!(session.title.as_deref(), Some("Fix the reboot bug"));
}

#[test]
fn dsh_plain_generation_zero_log_is_read() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("--home-me-proj--").join(DSH_ID);
    write_file(
        &dir.join("session.jsonl"),
        &format!("{}\n{DSH_USER}\n", dsh_header(DSH_ID)),
    );

    let sessions = dsh_sessions(root.path());

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title.as_deref(), Some("Fix the reboot bug"));
}

#[test]
fn dsh_log_being_written_still_yields_its_header() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("--home-me-proj--").join(DSH_ID);
    fs::create_dir_all(&dir).unwrap();
    let mut log = zstd::encode_all(format!("{}\n", dsh_header(DSH_ID)).as_bytes(), 3).unwrap();
    let batch = zstd::encode_all(format!("{DSH_USER}\n").as_bytes(), 3).unwrap();
    log.extend(&batch[..batch.len() / 2]);
    fs::write(dir.join("session.v4.jsonl.zstd"), log).unwrap();

    let sessions = dsh_sessions(root.path());

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, DSH_ID);
    assert_eq!(sessions[0].title, None);
}

#[test]
fn dsh_directories_without_a_log_or_outside_project_keys_are_skipped() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("--home-me-proj--").join(DSH_ID)).unwrap();
    write_file(
        &root.path().join("_no-cwd").join(DSH_ID).join("session.jsonl"),
        &format!("{}\n", dsh_header(DSH_ID)),
    );
    assert!(dsh_sessions(root.path()).is_empty());
}

#[test]
fn dsh_current_log_is_the_highest_generation() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "session.jsonl",
        "session.v2.jsonl.zstd",
        "session.v10.jsonl.zstd",
        "session.v9.jsonl",
        "session.v011.jsonl",
        "session.v0.jsonl",
        "session.lock",
        "notes.jsonl",
    ] {
        fs::write(dir.path().join(name), b"").unwrap();
    }
    assert_eq!(
        current_dsh_log(dir.path()),
        Some((dir.path().join("session.v10.jsonl.zstd"), true))
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp agent_session_history::tests::dsh`
Expected: compile error.

- [ ] **Step 3: Implement**

`app/Cargo.toml`: add `zstd = "0.13.3"` after `warp_files.workspace = true` (line 354, inside `[target.'cfg(not(target_family = "wasm"))'.dependencies]`).

`agent_session_paths.rs`: add after `kimi_home`:
```rust
/// DeepSeek Harness's data root: `$DSH_HOME`, else `~/.dsh`. dsh treats a blank value as unset.
pub(crate) fn dsh_home() -> Option<PathBuf> {
    std::env::var("DSH_HOME")
        .ok()
        .filter(|home| !home.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".dsh")))
}
```

`agent_session_history.rs`: extend the (non-wasm) import to `use super::agent_session_paths::{dsh_home, kimi_home, kimi_indexed_session_dirs};`, add `#[cfg(not(target_family = "wasm"))] use std::io::Read;`, add `#[cfg(not(target_family = "wasm"))] const DSH_HEAD_BYTES: u64 = 64 * 1024;` after `TITLE_MAX_CHARS`, and add after `kimi_session` (every item below is `#[cfg(not(target_family = "wasm"))]`, including `current_dsh_log`):
```rust
/// DeepSeek Harness's sessions under `root` (`<DSH_HOME>/sessions`), each described by the head
/// of its current log.
#[cfg(not(target_family = "wasm"))]
fn dsh_sessions(root: &Path) -> Vec<PastAgentSession> {
    directories(root)
        .into_iter()
        .filter(|project| {
            project
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.len() > 4 && name.starts_with("--") && name.ends_with("--"))
        })
        .flat_map(|project| directories(&project))
        .filter_map(|dir| dsh_session(&dir))
        .collect()
}

#[cfg(not(target_family = "wasm"))]
fn dsh_session(dir: &Path) -> Option<PastAgentSession> {
    let (path, compressed) = current_dsh_log(dir)?;
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    let head = read_dsh_head(&path, compressed);
    let mut rows = head
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok());
    let header = rows.next().filter(|header| header["type"] == "session")?;
    let session_id = header["id"]
        .as_str()
        .filter(|id| is_safe_session_id(id))?
        .to_owned();
    let cwd = header["cwd"].as_str().map(PathBuf::from).unwrap_or_default();
    let title = rows
        .find(|row| row["type"] == "user/message" && row["data"]["source"]["kind"] == "user")
        .and_then(|row| {
            row["data"]["content"]
                .as_array()?
                .iter()
                .find(|part| part["type"] == "text")?["text"]
                .as_str()
                .and_then(title)
        });
    Some(PastAgentSession {
        agent: CLIAgent::DeepSeekHarness,
        session_id,
        cwd,
        canonical_cwd: None,
        title,
        modified,
    })
}

/// The session's current log, the highest generation of `session[.vN].jsonl[.zstd]` in `dir`
/// (N has no leading zero, as dsh writes it), and whether it is zstd-compressed.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn current_dsh_log(dir: &Path) -> Option<(PathBuf, bool)> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let (stem, compressed) = match name.strip_suffix(".zstd") {
                Some(stem) => (stem, true),
                None => (name.as_str(), false),
            };
            let generation = match stem.strip_prefix("session")?.strip_suffix(".jsonl")? {
                "" => 0,
                version => {
                    let digits = version.strip_prefix(".v")?;
                    let canonical = !digits.starts_with('0')
                        && digits.bytes().all(|byte| byte.is_ascii_digit());
                    canonical.then(|| digits.parse::<u32>().ok()).flatten()?
                }
            };
            Some((generation, entry.path(), compressed))
        })
        .max_by_key(|(generation, _, _)| *generation)
        .map(|(_, path, compressed)| (path, compressed))
}

/// Up to [`DSH_HEAD_BYTES`] of the log's text. A log dsh is still appending to can end in a
/// partial frame; what decoded before it is kept.
#[cfg(not(target_family = "wasm"))]
fn read_dsh_head(path: &Path, compressed: bool) -> String {
    let mut head = Vec::new();
    if let Ok(file) = File::open(path) {
        let _ = if compressed {
            zstd::stream::read::Decoder::new(file)
                .and_then(|decoder| decoder.take(DSH_HEAD_BYTES).read_to_end(&mut head))
        } else {
            file.take(DSH_HEAD_BYTES).read_to_end(&mut head)
        };
    }
    String::from_utf8_lossy(&head).into_owned()
}
```
In `directory_sessions`, add before the `CLIAgent::Claude` group and remove `| CLIAgent::DeepSeekHarness` from it:
```rust
        CLIAgent::DeepSeekHarness => Some(
            dsh_home()
                .map(|home| dsh_sessions(&home.join("sessions")))
                .unwrap_or_default(),
        ),
```
(`read_to_end` appends what it read before an error, which is what keeps the header of a log mid-write. `.lines()` on the decoded text drops nothing but the torn final line, which then fails to parse and is skipped.)

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp agent_session_history`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add app/Cargo.toml Cargo.lock app/src/terminal/agent_session_paths.rs app/src/terminal/agent_session_history.rs app/src/terminal/agent_session_history_tests.rs
git commit -m "feat(cli-agent): list DeepSeek Harness sessions from their zstd logs" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---


### Task 7: Pane agent-event token and binary path in the shell environment

**Files:**
- Modify: `crates/warp_core/src/cli_agent_protocol.rs:14` (two constants)
- Create: `app/src/terminal/agent_event_token.rs`, `app/src/terminal/agent_event_token_tests.rs`
- Modify: `app/src/terminal/mod.rs:19` (module declaration)
- Modify: `app/src/terminal/local_tty/terminal_manager.rs:1856` (`create_pty`), `:1711` (after `active_session_id`)
- Modify: `app/src/terminal/writeable_pty/terminal_surface.rs:84` (new default method)
- Modify: `app/src/terminal/view.rs:2523` (field), `:4460` (init), `:26856` (accessors), `:27016` (surface impl)

**Interfaces:**
- Produces:
  - `warp_core::cli_agent_protocol::WARP_AGENT_EVENT_TOKEN_ENV = "WARP_AGENT_EVENT_TOKEN"`, `WARP_AGENT_EVENT_BIN_ENV = "WARP_AGENT_EVENT_BIN"`
  - `crate::terminal::agent_event_token::token_for(session_id: SessionId) -> Option<String>` (32 lowercase hex chars)
  - `crate::terminal::agent_event_token::add_env_vars(env_vars: &mut HashMap<OsString, OsString>, session_id: SessionId)`
  - `TerminalSurface::on_agent_event_token_determined(&mut self, token: String, ctx: &mut ViewContext<Self>)` (default no-op)
  - `TerminalView::agent_event_token(&self) -> Option<&str>`, `TerminalView::set_agent_event_token(&mut self, token: String)` (both `pub(crate)`, `#[cfg(feature = "local_tty")]`)

- [ ] **Step 1: Write the failing tests**

`app/src/terminal/agent_event_token_tests.rs`:
```rust
use std::collections::HashMap;
use std::ffi::OsString;

use warp_core::SessionId;
use warp_core::cli_agent_protocol::{WARP_AGENT_EVENT_BIN_ENV, WARP_AGENT_EVENT_TOKEN_ENV};

use super::{SECRET_LEN, insert_env_vars, load_or_create_secret, token_from_secret};

#[test]
fn token_is_128_bit_hex_and_stable_per_session() {
    let secret = [7u8; SECRET_LEN];
    let token = token_from_secret(&secret, SessionId::from(42));
    assert_eq!(token.len(), 32);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(token, token_from_secret(&secret, SessionId::from(42)));
}

#[test]
fn token_differs_by_session_and_by_secret() {
    let secret = [7u8; SECRET_LEN];
    let other_secret = [8u8; SECRET_LEN];
    let token = token_from_secret(&secret, SessionId::from(42));
    assert_ne!(token, token_from_secret(&secret, SessionId::from(43)));
    assert_ne!(token, token_from_secret(&other_secret, SessionId::from(42)));
}

#[test]
fn secret_is_created_once_and_then_reused() {
    let dir = tempfile::tempdir().unwrap();
    let first = load_or_create_secret(dir.path()).unwrap();
    let second = load_or_create_secret(dir.path()).unwrap();
    assert_eq!(first.len(), SECRET_LEN);
    assert_eq!(first, second);
}

#[test]
fn a_truncated_secret_file_is_not_used() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(super::SECRET_FILE_NAME), b"short").unwrap();
    assert!(load_or_create_secret(dir.path()).is_err());
}

#[test]
fn env_vars_carry_the_token_and_binary() {
    let mut env_vars = HashMap::new();
    insert_env_vars(
        &mut env_vars,
        "0123456789abcdef0123456789abcdef".to_owned(),
        std::path::Path::new(r"C:\Program Files\WarpSpeed\warpspeed.exe"),
    );
    assert_eq!(
        env_vars.get(&OsString::from(WARP_AGENT_EVENT_TOKEN_ENV)),
        Some(&OsString::from("0123456789abcdef0123456789abcdef"))
    );
    assert_eq!(
        env_vars.get(&OsString::from(WARP_AGENT_EVENT_BIN_ENV)),
        Some(&OsString::from(r"C:\Program Files\WarpSpeed\warpspeed.exe"))
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp terminal::agent_event_token`
Expected: compile error (module missing).

- [ ] **Step 3: Implement**

`crates/warp_core/src/cli_agent_protocol.rs`, after `WARP_CLIENT_VERSION_ENV`:
```rust
/// Environment variable carrying the token that addresses this pane for `warp agent-event`.
pub const WARP_AGENT_EVENT_TOKEN_ENV: &str = "WARP_AGENT_EVENT_TOKEN";

/// Environment variable carrying the absolute path of the Warp binary that accepts
/// `agent-event`, so hooks never hard-code an install path.
pub const WARP_AGENT_EVENT_BIN_ENV: &str = "WARP_AGENT_EVENT_BIN";
```

`app/src/terminal/agent_event_token.rs`:
```rust
//! The token that lets an agent hook outside the terminal address the pane its agent runs in.
//!
//! Derived from the shell's session id rather than stored with the pane: a pane that adopts a
//! supervised shell -- after a detach, a quit, or a crash of the window that started it -- knows
//! the session id but has no other way to learn what the shell's environment was given.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::sync::OnceLock;

use rand::RngCore;
use sha2::{Digest, Sha256};
use warp_core::SessionId;
use warp_core::cli_agent_protocol::{WARP_AGENT_EVENT_BIN_ENV, WARP_AGENT_EVENT_TOKEN_ENV};

const SECRET_FILE_NAME: &str = "agent-event-secret";
const SECRET_LEN: usize = 32;

/// The token for the shell session `session_id`, the same in every process of this profile.
pub(crate) fn token_for(session_id: SessionId) -> Option<String> {
    secret().map(|secret| token_from_secret(secret, session_id))
}

/// Adds the token and this binary's path for `session_id` to a shell's environment.
pub(crate) fn add_env_vars(env_vars: &mut HashMap<OsString, OsString>, session_id: SessionId) {
    let (Some(token), Ok(binary)) = (token_for(session_id), std::env::current_exe()) else {
        return;
    };
    insert_env_vars(env_vars, token, &binary);
}

fn insert_env_vars(env_vars: &mut HashMap<OsString, OsString>, token: String, binary: &Path) {
    env_vars.insert(WARP_AGENT_EVENT_TOKEN_ENV.into(), token.into());
    env_vars.insert(WARP_AGENT_EVENT_BIN_ENV.into(), binary.as_os_str().to_owned());
}

fn token_from_secret(secret: &[u8], session_id: SessionId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(session_id.as_u64().to_be_bytes());
    hex::encode(&hasher.finalize()[..16])
}

fn secret() -> Option<&'static [u8]> {
    static SECRET: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    SECRET
        .get_or_init(|| load_or_create_secret(&warp_core::paths::state_dir()).ok())
        .as_deref()
}

/// Reads the profile's secret, creating it on first use. Two processes creating it at once agree
/// on one: each writes its own temporary file and only the first hard link to the real name wins.
fn load_or_create_secret(dir: &Path) -> io::Result<Vec<u8>> {
    let path = dir.join(SECRET_FILE_NAME);
    match std::fs::read(&path) {
        Ok(secret) => return checked(secret),
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        Err(_) => {}
    }
    std::fs::create_dir_all(dir)?;
    let mut secret = vec![0u8; SECRET_LEN];
    rand::thread_rng().fill_bytes(&mut secret);
    let temporary = dir.join(format!("{SECRET_FILE_NAME}.{}.tmp", std::process::id()));
    std::fs::File::create(&temporary)?.write_all(&secret)?;
    let linked = std::fs::hard_link(&temporary, &path);
    let _ = std::fs::remove_file(&temporary);
    match linked {
        Ok(()) => Ok(secret),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            checked(std::fs::read(&path)?)
        }
        Err(error) => Err(error),
    }
}

fn checked(secret: Vec<u8>) -> io::Result<Vec<u8>> {
    if secret.len() == SECRET_LEN {
        Ok(secret)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the agent-event secret has the wrong length",
        ))
    }
}

#[cfg(test)]
#[path = "agent_event_token_tests.rs"]
mod tests;
```
`app/src/terminal/mod.rs`, before `pub(crate) mod agent_session_handoff;`:
```rust
#[cfg(feature = "local_tty")]
pub(crate) mod agent_event_token;
```

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp terminal::agent_event_token`
Expected: 5 passed.

- [ ] **Step 5: Wire it into the spawn path**

`terminal_manager.rs` `create_pty`, immediately before `let options = PtyOptions {`:
```rust
        let mut env_vars = env_vars;
        if FeatureFlag::HOANotifications.is_enabled() {
            agent_event_token::add_env_vars(&mut env_vars, persistent_session_id);
        }
```
(`persistent_session_id` here is the id the shell is spawned and supervised under; `FeatureFlag` is already imported at line 45.) Add `use crate::terminal::agent_event_token;` to the imports.

Right after `let active_session_id = pty.persistent_session_id().unwrap_or(generated_session_id);` (line 1711):
```rust
    if FeatureFlag::HOANotifications.is_enabled()
        && let Some(token) = agent_event_token::token_for(active_session_id)
    {
        manager.view.update(ctx, |surface, ctx| {
            surface.on_agent_event_token_determined(token, ctx);
        });
    }
```
`terminal_surface.rs`, after `set_pending_restore_command` (line 84):
```rust
    /// Called with the token the shell's agent hooks address this pane by, once the session the
    /// pane runs (or adopted) is known.
    #[cfg(feature = "local_tty")]
    fn on_agent_event_token_determined(&mut self, _token: String, _ctx: &mut ViewContext<Self>) {}
```
`view.rs`: field after `terminal_session_uuid_hex` (line 2523):
```rust
    /// What this pane's shell was given as `WARP_AGENT_EVENT_TOKEN`.
    #[cfg(feature = "local_tty")]
    agent_event_token: Option<String>,
```
init after `terminal_session_uuid_hex: None,` (line 4460): `#[cfg(feature = "local_tty")] agent_event_token: None,`; accessors after `set_terminal_session_uuid` (line 26858):
```rust
    #[cfg(feature = "local_tty")]
    pub(crate) fn agent_event_token(&self) -> Option<&str> {
        self.agent_event_token.as_deref()
    }

    #[cfg(feature = "local_tty")]
    pub(crate) fn set_agent_event_token(&mut self, token: String) {
        self.agent_event_token = Some(token);
    }
```
and in `impl TerminalSurface for TerminalView` (after `set_pending_restore_command`, line 27019):
```rust
    #[cfg(feature = "local_tty")]
    fn on_agent_event_token_determined(&mut self, token: String, _ctx: &mut ViewContext<Self>) {
        self.set_agent_event_token(token);
    }
```

- [ ] **Step 6: Build and commit**

Run: `cargo check -p warp` then `cargo nextest run -p warp terminal::agent_event_token local_tty::terminal_manager`
Expected: builds; tests pass.

```bash
git add crates/warp_core/src/cli_agent_protocol.rs app/src/terminal/agent_event_token.rs app/src/terminal/agent_event_token_tests.rs app/src/terminal/mod.rs app/src/terminal/local_tty/terminal_manager.rs app/src/terminal/writeable_pty/terminal_surface.rs app/src/terminal/view.rs
git commit -m "feat(terminal): give each local pane an agent-event token and binary path" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `Request::CliAgentEvent` — deliver an out-of-band event to the owning pane

**Files:**
- Modify: `app/src/remote_control/protocol.rs:17` (version), `:21-69` (`Request`)
- Modify: `app/src/remote_control/protocol_tests.rs:14-17`
- Modify: `app/src/remote_control/handler.rs:15-32` and new fns
- Modify: `app/src/remote_control/handler_tests.rs` (new tests)
- Modify: `app/src/remote_control/client.rs:72-91` (`call_within`)
- Modify: `app/src/terminal/model_events.rs:364-370` (new emit fn)
- Modify: `app/src/terminal/view.rs` (new `receive_out_of_band_cli_agent_event`)
- Modify: `app/src/workspace/view.rs` near `terminal_panes` (line 15044)

**Interfaces:**
- Consumes: Task 7 `TerminalView::{agent_event_token, set_agent_event_token}`; Task 2 `is_agent_supported(Kimi)`; Task 3 `KimiSessionHandler`.
- Produces:
  - `Request::CliAgentEvent { token: String, notification: String }`; answers `Response::Done` or `Response::Failed { error }`
  - `remote_control::protocol::PROTOCOL_VERSION == 3`
  - `Instance::call_within(&self, request: Request, timeout: Duration) -> anyhow::Result<Response>` (`pub(crate)`)
  - `ModelEventDispatcher::emit_out_of_band_cli_agent_notification(&mut self, body: String, ctx: &mut ModelContext<Self>)`
  - `TerminalView::receive_out_of_band_cli_agent_event(&mut self, notification: String, ctx: &mut ViewContext<Self>)`
  - `Workspace::terminal_view_with_agent_event_token(&self, token: &str, ctx: &AppContext) -> Option<ViewHandle<TerminalView>>`

- [ ] **Step 1: Write the failing tests**

`protocol_tests.rs`: replace `protocol_version_is_two` with
```rust
#[test]
fn protocol_version_is_three() {
    assert_eq!(PROTOCOL_VERSION, 3);
}

#[test]
fn cli_agent_event_round_trips() {
    assert_round_trips(Request::CliAgentEvent {
        token: "0123456789abcdef0123456789abcdef".to_owned(),
        notification: r#"{"v":1,"agent":"kimi","event":"stop"}"#.to_owned(),
    });
}
```
Append to `handler_tests.rs`:
```rust
const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const KIMI_SESSION_ID: &str = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";

fn deliver(token: &str, notification: &str, app: &mut App) -> Response {
    app.update(|ctx| {
        handle(
            Request::CliAgentEvent {
                token: token.to_owned(),
                notification: notification.to_owned(),
            },
            ctx,
        )
    })
}

#[test]
fn cli_agent_event_reaches_the_pane_that_owns_the_token() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        terminal_view.update(&mut app, |view, _| view.set_agent_event_token(TOKEN.to_owned()));

        let response = deliver(
            TOKEN,
            &format!(
                r#"{{"v":1,"agent":"kimi","event":"session_start","session_id":"{KIMI_SESSION_ID}","cwd":"/srv/app","transcript_path":"/home/ben/.kimi-code/sessions/wd_app_0123456789ab/{KIMI_SESSION_ID}"}}"#
            ),
            &mut app,
        );
        assert!(matches!(response, Response::Done), "{response:?}");

        let response = deliver(
            TOKEN,
            r#"{"v":1,"agent":"kimi","event":"prompt_submit","query":"fix the build"}"#,
            &mut app,
        );
        assert!(matches!(response, Response::Done), "{response:?}");

        app.read(|ctx| {
            let session = CLIAgentSessionsModel::as_ref(ctx)
                .session(terminal_view.id())
                .expect("the event created an agent session");
            assert_eq!(session.agent, CLIAgent::Kimi);
            assert_eq!(
                session.session_context.session_id.as_deref(),
                Some(KIMI_SESSION_ID)
            );
            assert_eq!(session.session_context.cwd.as_deref(), Some("/srv/app"));
            assert_eq!(
                session.session_context.query.as_deref(),
                Some("fix the build")
            );
            assert_eq!(session.status, CLIAgentSessionStatus::InProgress);
            assert!(session.received_rich_notification);
        });
    });
}

#[test]
fn cli_agent_event_with_an_unknown_token_is_refused() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        terminal_view.update(&mut app, |view, _| view.set_agent_event_token(TOKEN.to_owned()));

        let response = deliver(
            "ffffffffffffffffffffffffffffffff",
            r#"{"v":1,"agent":"kimi","event":"stop"}"#,
            &mut app,
        );
        assert!(matches!(response, Response::Failed { .. }), "{response:?}");
        app.read(|ctx| {
            assert!(
                CLIAgentSessionsModel::as_ref(ctx)
                    .session(terminal_view.id())
                    .is_none()
            );
        });
    });
}

#[test]
fn cli_agent_event_without_any_tokened_pane_is_refused() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let _workspace = mock_workspace(&mut app);
        let response = deliver("", r#"{"v":1,"agent":"kimi","event":"stop"}"#, &mut app);
        assert!(matches!(response, Response::Failed { .. }), "{response:?}");
    });
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp remote_control::`
Expected: compile error (`no variant CliAgentEvent`).

- [ ] **Step 3: Implement**

`protocol.rs`: `pub const PROTOCOL_VERSION: u32 = 3;` and append to `enum Request` (after `MoveStatus`):
```rust
    /// Delivers a CLI agent event that could not be written to its terminal, to the pane whose
    /// shell was given `token` as `WARP_AGENT_EVENT_TOKEN`.
    CliAgentEvent {
        token: String,
        /// One protocol v1 notification as JSON, exactly what would follow the OSC 777
        /// `warp://cli-agent` title. A string because the notification's `serde_json::Value`
        /// fields do not survive bincode.
        notification: String,
    },
```
`handler.rs` `handle`: add
```rust
        #[cfg(feature = "local_tty")]
        Request::CliAgentEvent {
            token,
            notification,
        } => deliver_cli_agent_event(&token, notification, ctx),
        #[cfg(not(feature = "local_tty"))]
        Request::CliAgentEvent { .. } => Response::Failed {
            error: "This build has no local terminals".to_owned(),
        },
```
and the functions:
```rust
/// Hands an out-of-band agent event to the pane whose shell carries `token`.
#[cfg(feature = "local_tty")]
fn deliver_cli_agent_event(token: &str, notification: String, ctx: &mut AppContext) -> Response {
    use warpui::SingletonEntity;

    let view = (!token.is_empty())
        .then(|| {
            crate::workspace::WorkspaceRegistry::as_ref(ctx)
                .all_workspaces(ctx)
                .into_iter()
                .find_map(|(_, workspace)| {
                    workspace
                        .as_ref(ctx)
                        .terminal_view_with_agent_event_token(token, ctx)
                })
        })
        .flatten();
    let Some(view) = view else {
        return Response::Failed {
            error: "No pane in this instance owns that agent-event token".to_owned(),
        };
    };
    view.update(ctx, |view, ctx| {
        view.receive_out_of_band_cli_agent_event(notification, ctx)
    });
    Response::Done
}
```
(Without `local_tty` the request is answered by the cfg-gated arm in `handle`, so no stub function with unused parameters exists.)

`model_events.rs`, after `request_remote_server_block`:
```rust
    /// Emits a structured CLI agent notification that arrived outside the PTY, so it reaches
    /// exactly the subscribers an OSC 777 one does.
    pub fn emit_out_of_band_cli_agent_notification(
        &mut self,
        body: String,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.emit(ModelEvent::PluggableNotification {
            title: Some(CLI_AGENT_NOTIFICATION_SENTINEL.to_owned()),
            body,
        });
    }
```
with `use warp_core::cli_agent_protocol::CLI_AGENT_NOTIFICATION_SENTINEL;` at the top.

`view.rs`, next to `handle_cli_agent_notification` (line 13549):
```rust
    /// Takes a CLI agent event that `warp agent-event` delivered for this pane and feeds it in
    /// where an OSC 777 event from the PTY would enter.
    pub(crate) fn receive_out_of_band_cli_agent_event(
        &mut self,
        notification: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.model_events_handle.update(ctx, |dispatcher, ctx| {
            dispatcher.emit_out_of_band_cli_agent_notification(notification, ctx);
        });
    }
```
`workspace/view.rs`, after `terminal_panes` (ends line 15134):
```rust
    /// The terminal view whose shell was given `token` as its agent-event token, in any tab of
    /// this window.
    #[cfg(feature = "local_tty")]
    pub(crate) fn terminal_view_with_agent_event_token(
        &self,
        token: &str,
        ctx: &AppContext,
    ) -> Option<ViewHandle<TerminalView>> {
        self.tabs.iter().find_map(|tab| {
            let pane_group = tab.pane_group.as_ref(ctx);
            pane_group.pane_ids().find_map(|pane_id| {
                pane_group
                    .terminal_view_from_pane_id(pane_id, ctx)
                    .filter(|view| view.as_ref(ctx).agent_event_token() == Some(token))
            })
        })
    }
```
`client.rs`: replace `call` with
```rust
    /// Sends one request and waits for the answer.
    pub(crate) fn call(&self, request: Request) -> Result<Response> {
        self.call_within(request, REQUEST_TIMEOUT)
    }

    /// Sends one request and waits at most `timeout` for the answer.
    pub(crate) fn call_within(&self, request: Request, timeout: Duration) -> Result<Response> {
        let caller = service_caller::<RemoteControlService>(self.client.clone());
        let answer = block_on(futures::future::select(
            Box::pin(caller.call(request)),
            Box::pin(warpui_core::r#async::Timer::after(timeout)),
        ));

        match answer {
            futures::future::Either::Left((result, _)) => {
                result.map_err(|error| anyhow::anyhow!("{error}"))
            }
            futures::future::Either::Right(_) => bail!(
                "The Warp instance with pid {} did not answer within {} seconds",
                self.marker.pid,
                timeout.as_secs()
            ),
        }
    }
```
`sessions_cli.rs` matches on `Request` nowhere exhaustively; if `cargo check` reports a non-exhaustive match anywhere else, add the arm there.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp remote_control::`
Expected: PASS (the three new handler tests, the protocol tests, and all existing ones).

- [ ] **Step 5: Commit**

```bash
git add app/src/remote_control/protocol.rs app/src/remote_control/protocol_tests.rs app/src/remote_control/handler.rs app/src/remote_control/handler_tests.rs app/src/remote_control/client.rs app/src/terminal/model_events.rs app/src/terminal/view.rs app/src/workspace/view.rs
git commit -m "feat(remote-control): deliver CLI agent events to the pane owning a token" -m "Bumps the remote-control protocol to 3: sessions commands of older builds no longer find newer instances, and the reverse." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: `warpspeed agent-event` — map a Kimi hook payload or pass a protocol event through, and deliver it

Kimi hook payload facts (`kimi-code/packages/agent-core-v2/src/features/externalHooks/…`): the command runs through Node `spawn(command, { shell: true })` (so `cmd.exe` on Windows), gets `JSON.stringify(input)` on stdin, **exit code 2 blocks the agent, and a `UserPromptSubmit` hook's stdout is injected into the prompt** — so this command must print nothing to stdout and exit 0. Top-level keys are snake_case (`hook_event_name`, `session_id`, `cwd`, `client_type`, …); nested objects (`display`, `tool_input`) keep their own keys. No hook carries a transcript path; only `SessionStart`/`SessionEnd` carry the session's cwd (agent-level hooks carry Kimi's process cwd). `UserPromptSubmit.prompt` is an array of content parts. `StopFailure` carries `error_type` and `error_message`.

**Files:**
- Create: `crates/warp_cli/src/agent_event.rs`
- Modify: `crates/warp_cli/src/lib.rs:20-45` (module), `:656-719` (`Command`), `:723-736` (`prints_to_stdout`)
- Test: `crates/warp_cli/src/lib_tests.rs`
- Modify: `app/src/terminal/agent_session_paths.rs` (+`_tests.rs`): `kimi_work_dir_key`
- Create: `app/src/agent_event_cli/mod.rs`, `app/src/agent_event_cli/mod_tests.rs`, `app/src/agent_event_cli/kimi_hook.rs`, `app/src/agent_event_cli/kimi_hook_tests.rs`
- Modify: `app/src/lib.rs:76` (module), `:861-875` (dispatch)

**Interfaces:**
- Consumes: Task 8 `Request::CliAgentEvent`, `Instance::call_within`; Task 5 `kimi_home`, `kimi_indexed_session_dirs`; Task 7 `WARP_AGENT_EVENT_TOKEN_ENV`.
- Produces:
  - `warp_cli::agent_event::{AgentEventArgs { format: AgentEventFormat, hooks_version: Option<String> }, AgentEventFormat::{KimiHook, Protocol}}`; `warp_cli::Command::AgentEvent(AgentEventArgs)` (hidden), CLI spelling `agent-event --format <kimi-hook|protocol> [--hooks-version <VERSION>]`
  - `crate::terminal::agent_session_paths::kimi_work_dir_key(work_dir: &str) -> String`
  - `crate::agent_event_cli::run(args: &AgentEventArgs)`

- [ ] **Step 1: Write the failing tests**

Append to `crates/warp_cli/src/lib_tests.rs`:
```rust
#[test]
fn agent_event_takes_one_of_two_formats() {
    for (spelling, expected) in [
        ("kimi-hook", agent_event::AgentEventFormat::KimiHook),
        ("protocol", agent_event::AgentEventFormat::Protocol),
    ] {
        let args = Args::try_parse_from(["warpspeed", "agent-event", "--format", spelling]).unwrap();
        match args.command() {
            Some(Command::AgentEvent(event)) => {
                assert_eq!(event.format, expected);
                assert_eq!(event.hooks_version, None);
                assert!(!Command::AgentEvent(event.clone()).prints_to_stdout());
            }
            other => panic!("expected agent-event, got {other:?}"),
        }
    }
    let versioned = Args::try_parse_from([
        "warpspeed",
        "agent-event",
        "--format",
        "kimi-hook",
        "--hooks-version",
        "1.0.0",
    ])
    .unwrap();
    let Some(Command::AgentEvent(event)) = versioned.command() else {
        panic!("expected agent-event");
    };
    assert_eq!(event.hooks_version.as_deref(), Some("1.0.0"));
    assert!(Args::try_parse_from(["warpspeed", "agent-event"]).is_err());
    assert!(Args::try_parse_from(["warpspeed", "agent-event", "--format", "osc"]).is_err());
}
```
Append to `app/src/terminal/agent_session_paths_tests.rs` (and add `kimi_work_dir_key` to its `use super::…`):
```rust
/// Vectors from Kimi's `encodeWorkDirKey` (`workdir-slug.ts`): sha256 over the path with `\`
/// turned into `/` and trailing `/` removed, case kept; the first one is Kimi's own test.
#[test]
fn kimi_work_dir_key_matches_kimi() {
    for (work_dir, key) in [
        ("/Users/me/Developer/proj", "wd_proj_76bfce3667dc"),
        (r"C:\Users\me\proj", "wd_proj_4045ff9b01fe"),
        (r"c:\users\me\proj", "wd_proj_80e537bce8d6"),
        ("/home/me/my project/", "wd_my-project_123dafdbc83f"),
        ("/", "wd_workspace_e3b0c44298fc"),
    ] {
        assert_eq!(kimi_work_dir_key(work_dir), key, "{work_dir}");
    }
}

#[test]
fn kimi_work_dir_slug_follows_kimi() {
    let slug = |name: &str| {
        let key = kimi_work_dir_key(&format!("/x/{name}"));
        key["wd_".len()..key.len() - "_0123456789ab".len()].to_owned()
    };
    assert_eq!(slug("Some Folder With Spaces"), "some-folder-with-spaces");
    assert_eq!(slug("--Hello__World!!--"), "hello__world");
    assert_eq!(slug(&"a".repeat(50)), "a".repeat(40));
    assert_eq!(slug(&format!("{}-{}", "a".repeat(39), "b")), "a".repeat(39));
    assert_eq!(slug(".."), "workspace");
    assert_eq!(slug("..."), "...");
}
```
`app/src/agent_event_cli/kimi_hook_tests.rs`:
```rust
use std::fs;

use serde_json::{Value, json};

use super::to_notification;
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::event::{
    CLI_AGENT_NOTIFICATION_SENTINEL, CLIAgentEvent, CLIAgentEventType, parse_event,
};

const SESSION_ID: &str = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";

/// A hook payload as Kimi writes it to the hook's stdin, with `extra` merged in.
fn hook(name: &str, extra: Value) -> Value {
    let mut payload = json!({
        "hook_event_name": name,
        "session_id": SESSION_ID,
        "cwd": r"C:\Users\me\proj",
        "client_type": "kimi_code_cli",
        "session_title": "Fix the build",
    });
    payload
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    payload
}

/// What Warp makes of the notification a hook produces, as if it had arrived over OSC 777.
fn parsed(payload: &Value) -> CLIAgentEvent {
    let notification = to_notification(payload, Some("1.0.0"), None).expect("the hook is tracked");
    parse_event(
        Some(CLI_AGENT_NOTIFICATION_SENTINEL),
        &serde_json::to_string(&notification).unwrap(),
    )
    .expect("Warp parses the notification")
}

#[test]
fn session_start_reports_the_session_its_directory_and_the_hooks_version() {
    let home = tempfile::tempdir().unwrap();
    let dir = home
        .path()
        .join("sessions")
        .join("wd_proj_4045ff9b01fe")
        .join(SESSION_ID);
    fs::create_dir_all(&dir).unwrap();
    let line = json!({"sessionId": SESSION_ID, "sessionDir": dir, "workDir": "C:/Users/me/proj"});
    fs::write(home.path().join("session_index.jsonl"), format!("{line}\n")).unwrap();
    let payload = hook(
        "SessionStart",
        json!({"source": "startup", "model": "kimi-k2", "profile": "default"}),
    );

    let notification = to_notification(&payload, Some("1.0.0"), Some(home.path())).unwrap();

    assert_eq!(notification.agent.as_deref(), Some("kimi"));
    assert_eq!(notification.event, "session_start");
    assert_eq!(notification.session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(notification.cwd.as_deref(), Some(r"C:\Users\me\proj"));
    assert_eq!(
        notification.transcript_path,
        Some(dir.display().to_string())
    );
    assert_eq!(notification.plugin_version.as_deref(), Some("1.0.0"));
    let event = parsed(&payload);
    assert_eq!(event.agent, CLIAgent::Kimi);
    assert_eq!(event.event, CLIAgentEventType::SessionStart);
}

#[test]
fn session_start_without_an_index_finds_the_directory_by_kimis_naming() {
    let home = tempfile::tempdir().unwrap();
    let dir = home
        .path()
        .join("sessions")
        .join("wd_proj_4045ff9b01fe")
        .join(SESSION_ID);
    fs::create_dir_all(&dir).unwrap();

    let notification = to_notification(
        &hook("SessionStart", json!({"source": "resume"})),
        None,
        Some(home.path()),
    )
    .unwrap();

    assert_eq!(
        notification.transcript_path,
        Some(dir.display().to_string())
    );
}

#[test]
fn session_start_omits_a_directory_that_does_not_exist() {
    let home = tempfile::tempdir().unwrap();
    let notification = to_notification(
        &hook("SessionStart", json!({"source": "startup"})),
        None,
        Some(home.path()),
    )
    .unwrap();
    assert_eq!(notification.transcript_path, None);
}

#[test]
fn prompt_submit_joins_the_prompts_text_parts_and_leaves_the_cwd_out() {
    let event = parsed(&hook(
        "UserPromptSubmit",
        json!({
            "prompt": [
                {"type": "text", "text": "Fix the build"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}},
                {"type": "text", "text": "and the tests"}
            ],
            "is_steer": false
        }),
    ));
    assert_eq!(event.event, CLIAgentEventType::PromptSubmit);
    assert_eq!(
        event.payload.query.as_deref(),
        Some("Fix the build\nand the tests")
    );
    assert_eq!(event.cwd, None);
}

#[test]
fn permission_request_carries_the_tool_and_a_summary() {
    let event = parsed(&hook(
        "PermissionRequest",
        json!({
            "id": "approval_1",
            "agent_id": "main",
            "turn_id": 3,
            "tool_call_id": "call_1",
            "tool_name": "Shell",
            "action": "run command",
            "display": {"kind": "command", "command": "cargo test", "description": "Run the tests"},
            "tool_input": {"command": "cargo test"}
        }),
    ));
    assert_eq!(event.event, CLIAgentEventType::PermissionRequest);
    assert_eq!(event.payload.tool_name.as_deref(), Some("Shell"));
    assert_eq!(event.payload.summary.as_deref(), Some("Run the tests"));
    assert_eq!(
        event.payload.tool_input_preview.as_deref(),
        Some("cargo test")
    );
}

#[test]
fn permission_summary_falls_back_through_the_display_to_the_action() {
    let file = parsed(&hook(
        "PermissionRequest",
        json!({
            "tool_name": "WriteFile",
            "action": "write file",
            "display": {"kind": "file_io", "operation": "write", "path": "src/main.rs"},
            "tool_input": {"path": "src/main.rs"}
        }),
    ));
    assert_eq!(file.payload.summary.as_deref(), Some("src/main.rs"));
    let todo = parsed(&hook(
        "PermissionRequest",
        json!({"tool_name": "Todo", "action": "update todos", "display": {"kind": "todo_list", "items": []}}),
    ));
    assert_eq!(todo.payload.summary.as_deref(), Some("update todos"));
}

#[test]
fn permission_result_tool_use_stop_and_interrupt_map_directly() {
    assert_eq!(
        parsed(&hook("PermissionResult", json!({"tool_name": "Shell", "decision": "approved"}))).event,
        CLIAgentEventType::PermissionReplied
    );
    let tool = parsed(&hook(
        "PostToolUse",
        json!({"tool_name": "Shell", "tool_input": {"command": "ls"}, "tool_call_id": "call_2", "tool_output": "a\nb"}),
    ));
    assert_eq!(tool.event, CLIAgentEventType::ToolComplete);
    assert_eq!(tool.payload.tool_name.as_deref(), Some("Shell"));
    assert_eq!(
        parsed(&hook("Stop", json!({"stop_hook_active": false}))).event,
        CLIAgentEventType::Stop
    );
    assert_eq!(
        parsed(&hook("Interrupt", json!({"turn_id": 3, "reason": "cancelled"}))).event,
        CLIAgentEventType::IdlePrompt
    );
}

#[test]
fn stop_failure_carries_the_error() {
    let event = parsed(&hook(
        "StopFailure",
        json!({"error_type": "RateLimitError", "error_message": "429 Too Many Requests"}),
    ));
    assert_eq!(event.event, CLIAgentEventType::StopFailure);
    assert_eq!(event.payload.error_type.as_deref(), Some("RateLimitError"));
    assert_eq!(
        event.payload.response.as_deref(),
        Some("429 Too Many Requests")
    );
}

#[test]
fn untracked_hooks_send_nothing() {
    for name in [
        "SessionEnd",
        "PreToolUse",
        "PostToolUseFailure",
        "UserPromptQueued",
        "TurnStarted",
        "SessionHeartbeat",
        "SubagentStart",
        "SubagentStop",
        "TaskStarted",
        "PreCompact",
        "PostCompact",
        "Notification",
    ] {
        assert!(
            to_notification(&hook(name, json!({})), None, None).is_none(),
            "{name}"
        );
    }
    assert!(to_notification(&json!({"session_id": SESSION_ID}), None, None).is_none());
}
```
`app/src/agent_event_cli/mod_tests.rs`:
```rust
use warp_cli::agent_event::AgentEventFormat;

use super::notification_json;
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::event::{
    CLI_AGENT_NOTIFICATION_SENTINEL, CLIAgentEventType, parse_event,
};

#[test]
fn protocol_input_passes_through_unchanged_in_meaning() {
    let input = r#"{"v":1,"agent":"dsh","event":"tool_complete","session_id":"session-1","tool_name":"bash","tool_input":{"command":"ls"},"plugin_version":"0.1.0"}"#;

    let json = notification_json(AgentEventFormat::Protocol, input, None).expect("valid input");

    let event = parse_event(Some(CLI_AGENT_NOTIFICATION_SENTINEL), &json).expect("Warp parses it");
    assert_eq!(event.agent, CLIAgent::DeepSeekHarness);
    assert_eq!(event.event, CLIAgentEventType::ToolComplete);
    assert_eq!(event.session_id.as_deref(), Some("session-1"));
    assert_eq!(event.payload.tool_input_preview.as_deref(), Some("ls"));
    assert_eq!(event.payload.plugin_version.as_deref(), Some("0.1.0"));
}

#[test]
fn kimi_hook_input_is_mapped_with_its_hooks_version() {
    let input = r#"{"hook_event_name":"Stop","session_id":"session_x","cwd":"/p","client_type":"kimi_code_cli","stop_hook_active":false}"#;

    let json = notification_json(AgentEventFormat::KimiHook, input, Some("1.0.0")).unwrap();

    let event = parse_event(Some(CLI_AGENT_NOTIFICATION_SENTINEL), &json).unwrap();
    assert_eq!(event.agent, CLIAgent::Kimi);
    assert_eq!(event.event, CLIAgentEventType::Stop);
    assert_eq!(event.payload.plugin_version.as_deref(), Some("1.0.0"));
}

#[test]
fn malformed_or_untracked_input_sends_nothing() {
    assert!(notification_json(AgentEventFormat::Protocol, "not json", None).is_none());
    assert!(notification_json(AgentEventFormat::Protocol, r#"{"v":1}"#, None).is_none());
    assert!(notification_json(AgentEventFormat::KimiHook, "{", None).is_none());
    assert!(
        notification_json(
            AgentEventFormat::KimiHook,
            r#"{"hook_event_name":"PreToolUse","session_id":"s"}"#,
            None
        )
        .is_none()
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp_cli agent_event` and `cargo nextest run -p warp agent_event_cli agent_session_paths`
Expected: compile errors.

- [ ] **Step 3: Implement the arguments**

`crates/warp_cli/src/agent_event.rs`:
```rust
//! Arguments for `agent-event`, which forwards one CLI agent event to the Warp pane its agent
//! runs in.
//!
//! Only the argument shapes live here; the app crate maps and delivers the event.

use clap::{Args, ValueEnum};

/// `agent-event --format <FORMAT>`, reading one JSON object from standard input.
#[derive(Debug, Clone, Args)]
pub struct AgentEventArgs {
    /// The shape of the JSON object on standard input.
    #[arg(long, value_enum)]
    pub format: AgentEventFormat,

    /// The version of the Warp hooks running this command, reported as their plugin version.
    #[arg(long, value_name = "VERSION")]
    pub hooks_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentEventFormat {
    /// A Kimi Code hook payload.
    KimiHook,
    /// A Warp CLI agent protocol v1 notification.
    Protocol,
}
```
`lib.rs`: add `#[cfg(not(target_family = "wasm"))] pub mod agent_event;` before `pub mod agent;` (line 22); in `enum Command` after `AgentCtrl(…)`:
```rust
    /// Forward one CLI agent event, read as JSON from standard input, to the Warp pane its agent
    /// runs in. Run by agent hooks and plugins that cannot write to the terminal.
    #[cfg(not(target_family = "wasm"))]
    #[command(hide = true)]
    AgentEvent(agent_event::AgentEventArgs),
```
and in `prints_to_stdout`:
```rust
            #[cfg(not(target_family = "wasm"))]
            Command::AgentEvent(_) => false,
```
(`false` keeps Windows from attaching a console, and the command prints nothing.)

- [ ] **Step 4: Implement `kimi_work_dir_key`**

Append to `app/src/terminal/agent_session_paths.rs` (with `use sha2::{Digest, Sha256};` at the top):
```rust
/// The directory Kimi names a working directory's sessions after (`encodeWorkDirKey` in
/// `agent-core-v2/src/_base/utils/workdir-slug.ts`): `wd_<slug>_<first 12 hex of sha256>`.
/// Kimi reuses an existing workspace whose root differs only in case, so its session index is
/// the better source when it has the session.
pub(crate) fn kimi_work_dir_key(work_dir: &str) -> String {
    let normalized = work_dir.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let base = normalized.rsplit('/').next().unwrap_or(normalized);
    let hash = hex::encode(Sha256::digest(normalized.as_bytes()));
    format!("wd_{}_{}", kimi_work_dir_slug(base), &hash[..12])
}

fn kimi_work_dir_slug(name: &str) -> String {
    const MAX_SLUG_LEN: usize = 40;
    let mut slug = String::new();
    let mut in_replaced_run = false;
    for character in name.to_lowercase().chars() {
        if character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '.' | '_' | '-')
        {
            slug.push(character);
            in_replaced_run = false;
        } else if !in_replaced_run {
            slug.push('-');
            in_replaced_run = true;
        }
    }
    let slug = slug.trim_matches('-');
    let slug = slug.get(..MAX_SLUG_LEN).unwrap_or(slug).trim_matches('-');
    match slug {
        "" | "." | ".." => "workspace".to_owned(),
        slug => slug.to_owned(),
    }
}
```

- [ ] **Step 5: Implement the mapping**

`app/src/agent_event_cli/kimi_hook.rs`:
```rust
//! Kimi Code hook payloads, as its `[[hooks]]` commands receive them on stdin, mapped to Warp's
//! CLI agent protocol.
//!
//! Kimi converts only top-level keys to snake_case; nested objects such as `display` and
//! `tool_input` keep the keys their tool gave them.

use std::path::{Path, PathBuf};

use serde_json::Value;
use warp_core::cli_agent_protocol::CLIAgentNotification;

use crate::terminal::agent_session_history::is_safe_session_id;
use crate::terminal::agent_session_paths::{kimi_indexed_session_dirs, kimi_work_dir_key};

/// The `agent` name events carry: `CLIAgent::Kimi`'s command prefix.
const AGENT: &str = "kimi";

/// Fields of a permission request's `display`, in the order the first one present summarises it.
const DISPLAY_SUMMARY_FIELDS: &[&str] = &[
    "description",
    "command",
    "path",
    "query",
    "url",
    "summary",
    "skill_name",
    "agent_name",
];

/// The notification for one hook payload, or `None` for hooks Warp does not track -- including
/// `SessionEnd`, since a pane's agent session ends when the `kimi` process does.
pub(super) fn to_notification(
    payload: &Value,
    hooks_version: Option<&str>,
    kimi_home: Option<&Path>,
) -> Option<CLIAgentNotification> {
    let base = |event: &str| CLIAgentNotification {
        session_id: text(payload, "session_id"),
        plugin_version: hooks_version.map(str::to_owned),
        ..CLIAgentNotification::new(AGENT, event)
    };
    let notification = match payload["hook_event_name"].as_str()? {
        "SessionStart" => {
            // Only session-level hooks carry the session's directory; the others carry the
            // directory Kimi itself runs in.
            let cwd = text(payload, "cwd");
            let transcript_path = kimi_home
                .zip(text(payload, "session_id"))
                .and_then(|(home, session_id)| session_dir(home, &session_id, cwd.as_deref()))
                .map(|dir| dir.display().to_string());
            CLIAgentNotification {
                cwd,
                transcript_path,
                ..base("session_start")
            }
        }
        "UserPromptSubmit" => CLIAgentNotification {
            query: prompt_text(&payload["prompt"]),
            ..base("prompt_submit")
        },
        "PermissionRequest" => CLIAgentNotification {
            tool_name: text(payload, "tool_name"),
            tool_input: tool_input(payload),
            summary: permission_summary(payload),
            ..base("permission_request")
        },
        "PermissionResult" => base("permission_replied"),
        "PostToolUse" => CLIAgentNotification {
            tool_name: text(payload, "tool_name"),
            tool_input: tool_input(payload),
            ..base("tool_complete")
        },
        "Stop" => base("stop"),
        "StopFailure" => CLIAgentNotification {
            error_type: text(payload, "error_type"),
            response: text(payload, "error_message"),
            ..base("stop_failure")
        },
        "Interrupt" => base("idle_prompt"),
        _ => return None,
    };
    Some(notification)
}

fn text(payload: &Value, key: &str) -> Option<String> {
    payload[key]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The text of a prompt, which Kimi sends as content parts.
fn prompt_text(prompt: &Value) -> Option<String> {
    let text = match prompt.as_str() {
        Some(text) => text.to_owned(),
        None => prompt
            .as_array()?
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn tool_input(payload: &Value) -> Option<Value> {
    Some(payload["tool_input"].clone()).filter(|input| !input.is_null())
}

fn permission_summary(payload: &Value) -> Option<String> {
    DISPLAY_SUMMARY_FIELDS
        .iter()
        .find_map(|field| text(&payload["display"], field))
        .or_else(|| text(payload, "action"))
}

/// The session's directory, from Kimi's index, else where Kimi's naming puts it for `cwd`;
/// `None` unless it exists.
fn session_dir(home: &Path, session_id: &str, cwd: Option<&str>) -> Option<PathBuf> {
    if !is_safe_session_id(session_id) {
        return None;
    }
    kimi_indexed_session_dirs(home)
        .and_then(|dirs| {
            dirs.into_iter()
                .find(|(id, _)| id == session_id)
                .map(|(_, dir)| dir)
        })
        .or_else(|| {
            Some(
                home.join("sessions")
                    .join(kimi_work_dir_key(cwd?))
                    .join(session_id),
            )
        })
        .filter(|dir| dir.is_dir())
}

#[cfg(test)]
#[path = "kimi_hook_tests.rs"]
mod tests;
```
(The `_` arm matches hook names, which are strings.)

- [ ] **Step 6: Implement delivery and the command**

`app/src/agent_event_cli/mod.rs`:
```rust
//! `<warp binary> agent-event` -- how an agent hook or plugin that cannot write to its terminal
//! reports a CLI agent event to the pane its agent runs in.
//!
//! It never fails, never prints, and never takes long. A hook's exit status and output feed back
//! into the agent -- Kimi Code blocks on exit code 2 and adds a `UserPromptSubmit` hook's stdout
//! to the prompt -- so every problem ends in a silent exit within [`DEADLINE`].

mod kimi_hook;

use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

use warp_cli::agent_event::{AgentEventArgs, AgentEventFormat};
use warp_core::cli_agent_protocol::{CLIAgentNotification, WARP_AGENT_EVENT_TOKEN_ENV};

use crate::remote_control::address;
use crate::remote_control::client::Instance;
use crate::remote_control::protocol::{Request, Response};
use crate::terminal::agent_session_paths::kimi_home;

const DEADLINE: Duration = Duration::from_secs(2);
const MAX_INPUT_BYTES: u64 = 1024 * 1024;

/// Forwards the event on stdin to the pane whose shell carries `WARP_AGENT_EVENT_TOKEN`.
pub(crate) fn run(args: &AgentEventArgs) {
    let Some(token) = std::env::var(WARP_AGENT_EVENT_TOKEN_ENV)
        .ok()
        .filter(|token| !token.is_empty())
    else {
        return;
    };
    let format = args.format;
    let hooks_version = args.hooks_version.clone();
    let (finished, done) = mpsc::channel();
    // On a thread so a stdin that never closes or an instance that never answers cannot hold
    // the agent up past the deadline.
    std::thread::spawn(move || {
        let mut input = String::new();
        if std::io::stdin()
            .take(MAX_INPUT_BYTES)
            .read_to_string(&mut input)
            .is_ok()
            && let Some(notification) = notification_json(format, &input, hooks_version.as_deref())
        {
            deliver(&token, &notification);
        }
        let _ = finished.send(());
    });
    let _ = done.recv_timeout(DEADLINE);
}

/// The protocol notification `input` describes, as JSON, or `None` when there is nothing to send.
fn notification_json(
    format: AgentEventFormat,
    input: &str,
    hooks_version: Option<&str>,
) -> Option<String> {
    let notification = match format {
        AgentEventFormat::KimiHook => kimi_hook::to_notification(
            &serde_json::from_str(input).ok()?,
            hooks_version,
            kimi_home().as_deref(),
        )?,
        AgentEventFormat::Protocol => serde_json::from_str::<CLIAgentNotification>(input).ok()?,
    };
    serde_json::to_string(&notification).ok()
}

/// Offers the notification to each running instance until one owns the token.
fn deliver(token: &str, notification: &str) {
    for marker in address::read_markers() {
        let Ok(instance) = Instance::connect(marker) else {
            continue;
        };
        let request = Request::CliAgentEvent {
            token: token.to_owned(),
            notification: notification.to_owned(),
        };
        if matches!(instance.call_within(request, DEADLINE), Ok(Response::Done)) {
            return;
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
```
`app/src/lib.rs`: after `mod remote_control;` (line 76) add
```rust
#[cfg(not(target_family = "wasm"))]
mod agent_event_cli;
```
and in the command dispatch, after the `warp_cli::Command::Sessions(args)` arm:
```rust
            #[cfg(not(target_family = "wasm"))]
            warp_cli::Command::AgentEvent(args) => {
                agent_event_cli::run(args);
                return Ok(());
            }
```
If `crate::terminal::agent_session_paths` is not reachable from `agent_event_cli` under `wasm`, that is fine: both are non-wasm here (`agent_session_paths` is unconditional).

- [ ] **Step 7: Run the tests**

Run: `cargo nextest run -p warp_cli agent_event` then `cargo nextest run -p warp agent_event_cli agent_session_paths`
Expected: PASS.

- [ ] **Step 8: Check the command is silent**

Run (Git Bash, from the worktree; `cargo run` starts the CLI, and nothing runs from `target/`):
```bash
cargo build -p warp --bin warpspeed
printf '%s' '{"hook_event_name":"Stop","session_id":"session_x"}' | WARP_AGENT_EVENT_TOKEN=00000000000000000000000000000000 cargo run -q -p warp --bin warpspeed -- agent-event --format kimi-hook 2>/dev/null | wc -c; echo "exit ${PIPESTATUS[1]}"
```
(The build first makes `cargo run` start at once, so the timing below measures the command, not the compile.)
Expected: `0` bytes on stdout, `exit 0`, returns in under 3 s. stderr carries only the `WarpSpeed starting:` banner.

- [ ] **Step 9: Commit**

```bash
git add crates/warp_cli/src/agent_event.rs crates/warp_cli/src/lib.rs crates/warp_cli/src/lib_tests.rs app/src/terminal/agent_session_paths.rs app/src/terminal/agent_session_paths_tests.rs app/src/agent_event_cli/mod.rs app/src/agent_event_cli/mod_tests.rs app/src/agent_event_cli/kimi_hook.rs app/src/agent_event_cli/kimi_hook_tests.rs app/src/lib.rs
git commit -m "feat(cli): add agent-event to forward hook and plugin events to their pane" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Feature flags and the Kimi Code hook installer

Kimi config facts: `<KIMI_CODE_HOME>/config.toml`, TOML; `[[hooks]]` entries are zod `.strict()` with only `event` (one of 20 names), `matcher?` (JS regex), `command` (non-empty), `timeout?` (integer seconds 1–600, default 30). One bad entry makes Kimi ignore the whole `hooks` section (with a warning); other sections are unaffected.

**Files:**
- Modify: `crates/warp_features/src/lib.rs:806` (variants), `:1048` (`DOGFOOD_FLAGS`)
- Create: `app/src/terminal/cli_agent_sessions/plugin_manager/kimi.rs`, `.../plugin_manager/kimi_tests.rs`
- Modify: `app/src/terminal/cli_agent_sessions/plugin_manager/mod.rs:1-15, 256-305`
- Modify: `app/src/terminal/cli_agent_sessions/plugin_manager/mod_tests.rs`

**Interfaces:**
- Consumes: Task 5 `kimi_home`; Task 9 `--hooks-version` (the hooks report `HOOKS_VERSION` as their plugin version, which the footer's update check compares with `minimum_plugin_version()`).
- Produces: `FeatureFlag::KimiNotifications`, `FeatureFlag::DshNotifications` (both in `DOGFOOD_FLAGS`); `plugin_manager_for(CLIAgent::Kimi)` → `KimiPluginManager` when `KimiNotifications && HOANotifications`; private fns `with_warp_hooks(config: &str, command: &str) -> Result<String, String>`, `installed_hooks_version(config: &str) -> Option<String>`, `hooks_need_update(config: &str) -> bool`, `hook_command() -> String`; consts `HOOKS_VERSION = "1.0.0"`, `HOOK_EVENTS`, `MANUAL_HOOK_EXAMPLE`.

- [ ] **Step 1: Write the failing tests**

`plugin_manager/kimi_tests.rs`:
```rust
use toml_edit::DocumentMut;

use super::{
    HOOK_EVENTS, HOOKS_VERSION, MANUAL_HOOK_EXAMPLE, hook_command, hooks_need_update,
    installed_hooks_version, with_warp_hooks,
};

/// Kimi's `HooksConfigSchema` (`externalHooks/configSection.ts:10-21`, zod `.strict()`).
const KIMI_HOOK_KEYS: &[&str] = &["event", "matcher", "command", "timeout"];
/// Kimi's `HOOK_EVENT_TYPES` (`externalHooks/internal/types.ts:3-24`).
const KIMI_HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionResult",
    "UserPromptSubmit",
    "UserPromptQueued",
    "TurnStarted",
    "Stop",
    "StopFailure",
    "Interrupt",
    "SessionStart",
    "SessionEnd",
    "SessionHeartbeat",
    "SubagentStart",
    "SubagentStop",
    "TaskStarted",
    "PreCompact",
    "PostCompact",
    "Notification",
];

/// Fails where Kimi would ignore every hook in `config`.
fn assert_kimi_accepts(config: &str) {
    let document = config.parse::<DocumentMut>().expect("valid TOML");
    let hooks = document["hooks"]
        .as_array_of_tables()
        .expect("hooks are [[hooks]] tables");
    for hook in hooks.iter() {
        for (key, _) in hook.iter() {
            assert!(KIMI_HOOK_KEYS.contains(&key), "Kimi rejects hook key {key:?}");
        }
        let event = hook["event"].as_str().expect("event is a string");
        assert!(KIMI_HOOK_EVENTS.contains(&event), "unknown event {event}");
        assert!(!hook["command"].as_str().expect("command is a string").is_empty());
        if let Some(timeout) = hook.get("timeout") {
            let timeout = timeout.as_integer().expect("timeout is an integer");
            assert!((1..=600).contains(&timeout), "timeout {timeout}");
        }
    }
}

/// The events of the hooks that run the current Warp command.
fn warp_hook_events(config: &str) -> Vec<String> {
    let document = config.parse::<DocumentMut>().unwrap();
    document["hooks"]
        .as_array_of_tables()
        .unwrap()
        .iter()
        .filter(|hook| hook["command"].as_str() == Some(hook_command().as_str()))
        .map(|hook| hook["event"].as_str().unwrap().to_owned())
        .collect()
}

const USER_CONFIG: &str = r#"# my settings
default_model = "kimi-k2"   # keep this comment

[[hooks]]
event = "PreToolUse"
matcher = "Shell"
command = "python ~/guard.py"
timeout = 10
"#;

#[test]
fn installs_one_hook_per_event_into_an_empty_config() {
    let config = with_warp_hooks("", &hook_command()).unwrap();
    assert_kimi_accepts(&config);
    assert_eq!(warp_hook_events(&config), HOOK_EVENTS);
    assert_eq!(installed_hooks_version(&config).as_deref(), Some(HOOKS_VERSION));
    assert!(!hooks_need_update(&config));
}

#[test]
fn keeps_the_users_settings_hooks_and_comments() {
    let config = with_warp_hooks(USER_CONFIG, &hook_command()).unwrap();
    assert!(
        config.starts_with("# my settings\ndefault_model = \"kimi-k2\"   # keep this comment\n"),
        "{config}"
    );
    assert!(
        config.contains(
            "[[hooks]]\nevent = \"PreToolUse\"\nmatcher = \"Shell\"\ncommand = \"python ~/guard.py\"\ntimeout = 10\n"
        ),
        "{config}"
    );
    assert_kimi_accepts(&config);
    assert_eq!(warp_hook_events(&config), HOOK_EVENTS);
}

#[test]
fn installing_twice_changes_nothing() {
    let once = with_warp_hooks(USER_CONFIG, &hook_command()).unwrap();
    let twice = with_warp_hooks(&once, &hook_command()).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn updating_replaces_older_warp_hooks_and_keeps_others() {
    let old = concat!(
        "[[hooks]]\n",
        "event = \"Stop\"\n",
        "command = \"\\\"$WARP_AGENT_EVENT_BIN\\\" agent-event --format kimi-hook --hooks-version 0.9.0\"\n",
        "timeout = 5\n",
        "\n",
        "[[hooks]]\n",
        "event = \"Notification\"\n",
        "command = \"notify-send kimi\"\n",
    );
    assert_eq!(installed_hooks_version(old).as_deref(), Some("0.9.0"));
    assert!(hooks_need_update(old));

    let config = with_warp_hooks(old, &hook_command()).unwrap();

    assert!(!config.contains("0.9.0"), "{config}");
    assert!(config.contains("notify-send kimi"), "{config}");
    assert_eq!(warp_hook_events(&config), HOOK_EVENTS);
    assert!(!hooks_need_update(&config));
}

#[test]
fn a_missing_event_needs_an_update() {
    let partial = format!(
        "[[hooks]]\nevent = \"Stop\"\ncommand = {:?}\ntimeout = 5\n",
        hook_command()
    );
    assert_eq!(installed_hooks_version(&partial).as_deref(), Some(HOOKS_VERSION));
    assert!(hooks_need_update(&partial));
}

#[test]
fn a_config_without_warp_hooks_is_not_installed() {
    assert_eq!(installed_hooks_version(USER_CONFIG), None);
    assert!(!hooks_need_update(USER_CONFIG));
    assert_eq!(installed_hooks_version(""), None);
}

#[test]
fn refuses_hooks_that_are_not_tables_and_invalid_toml() {
    assert!(with_warp_hooks("hooks = []\n", &hook_command()).is_err());
    assert!(with_warp_hooks("this is = = not toml", &hook_command()).is_err());
}

#[test]
fn the_manual_example_is_the_installed_command() {
    let document = MANUAL_HOOK_EXAMPLE
        .parse::<DocumentMut>()
        .expect("the example is valid TOML");
    assert_eq!(
        document["hooks"][0]["command"].as_str(),
        Some(hook_command().as_str())
    );
    assert_kimi_accepts(MANUAL_HOOK_EXAMPLE);
}
```
Append to `plugin_manager/mod_tests.rs`:
```rust
#[test]
fn returns_manager_for_kimi() {
    let _hoa_guard = crate::features::FeatureFlag::HOANotifications.override_enabled(true);
    let _kimi_guard = crate::features::FeatureFlag::KimiNotifications.override_enabled(true);
    assert!(plugin_manager_for(CLIAgent::Kimi).is_some());
}

#[test]
fn returns_none_for_kimi_and_deepseek_harness_without_their_flags() {
    let _hoa_guard = crate::features::FeatureFlag::HOANotifications.override_enabled(true);
    let _kimi_guard = crate::features::FeatureFlag::KimiNotifications.override_enabled(false);
    let _dsh_guard = crate::features::FeatureFlag::DshNotifications.override_enabled(false);
    assert!(plugin_manager_for(CLIAgent::Kimi).is_none());
    assert!(plugin_manager_for(CLIAgent::DeepSeekHarness).is_none());
}
```
(The DeepSeek Harness manager is asserted in Task 12, where it is created.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp plugin_manager::kimi`
Expected: compile error.

- [ ] **Step 3: Feature flags**

`crates/warp_features/src/lib.rs`, after `GeminiNotifications,` (line 806):
```rust

    /// Enables the install/update chip for the Kimi Code Warp hooks.
    /// Requires HOANotifications to also be enabled.
    KimiNotifications,

    /// Enables the install/update chip for the DeepSeek Harness Warp plugin.
    /// Requires HOANotifications to also be enabled.
    DshNotifications,
```
and in `DOGFOOD_FLAGS` after `FeatureFlag::GeminiNotifications,` (line 1048): `FeatureFlag::KimiNotifications,` and `FeatureFlag::DshNotifications,`.

- [ ] **Step 4: The installer**

`plugin_manager/kimi.rs`:
```rust
//! Installs the Kimi Code hooks that report to Warp through `warp agent-event`.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::{fs, io};

use async_trait::async_trait;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use super::{
    CliAgentPluginManager, PluginInstallError, PluginInstructionStep, PluginInstructions,
    compare_versions,
};
use crate::terminal::agent_session_paths::kimi_home;

// The command's pieces are macros so `MANUAL_HOOK_EXAMPLE` can `concat!` them: one spelling of the
// command serves both the installed hooks and the manual instructions.
macro_rules! hooks_version {
    () => {
        "1.0.0"
    };
}
macro_rules! command_marker {
    () => {
        "agent-event --format kimi-hook"
    };
}
#[cfg(windows)]
macro_rules! agent_event_variable {
    () => {
        "%WARP_AGENT_EVENT_BIN%"
    };
}
#[cfg(not(windows))]
macro_rules! agent_event_variable {
    () => {
        "$WARP_AGENT_EVENT_BIN"
    };
}

/// Bumped whenever the hook entries change. Each hook reports it as its plugin version.
const HOOKS_VERSION: &str = hooks_version!();

const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    "PermissionResult",
    "PostToolUse",
    "Stop",
    "StopFailure",
    "Interrupt",
];

/// What marks a hook entry as Warp's, whichever version wrote it.
const COMMAND_MARKER: &str = command_marker!();

const HOOK_TIMEOUT_SECONDS: i64 = 5;

/// Kimi runs hook commands through `cmd.exe` on Windows and `/bin/sh` elsewhere. The variable is
/// empty outside a Warp pane, so the hook fails there and Kimi carries on.
const AGENT_EVENT_BIN: &str = concat!("\"", agent_event_variable!(), "\"");

/// The same command as [`hook_command`], as a TOML entry for the manual instructions.
const MANUAL_HOOK_EXAMPLE: &str = concat!(
    "[[hooks]]\nevent = \"Stop\"\ncommand = \"\\\"",
    agent_event_variable!(),
    "\\\" ",
    command_marker!(),
    " --hooks-version ",
    hooks_version!(),
    "\"\ntimeout = 5"
);

fn hook_command() -> String {
    format!("{AGENT_EVENT_BIN} {COMMAND_MARKER} --hooks-version {HOOKS_VERSION}")
}

pub(super) struct KimiPluginManager;

#[async_trait]
impl CliAgentPluginManager for KimiPluginManager {
    fn minimum_plugin_version(&self) -> &'static str {
        HOOKS_VERSION
    }

    fn can_auto_install(&self) -> bool {
        true
    }

    fn is_installed(&self) -> bool {
        read_config().is_some_and(|config| installed_hooks_version(&config).is_some())
    }

    fn needs_update(&self) -> bool {
        read_config().is_some_and(|config| hooks_need_update(&config))
    }

    async fn install(&self) -> Result<(), PluginInstallError> {
        write_hooks()
    }

    async fn update(&self) -> Result<(), PluginInstallError> {
        write_hooks()
    }

    fn install_success_message(&self) -> &'static str {
        "Warp hooks installed. Restart Kimi Code to activate."
    }

    fn update_success_message(&self) -> &'static str {
        "Warp hooks updated. Restart Kimi Code to activate."
    }

    fn install_instructions(&self) -> &'static PluginInstructions {
        &INSTRUCTIONS
    }

    fn update_instructions(&self) -> &'static PluginInstructions {
        &INSTRUCTIONS
    }
}

static INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: "Install Warp Hooks for Kimi Code",
    subtitle: "Warp adds hooks to Kimi Code's config.toml that report the session's status to \
               this pane. Your other settings and hooks are kept.",
    steps: &[PluginInstructionStep {
        description: "To add them by hand, put one entry like this per event (SessionStart, \
                      UserPromptSubmit, PermissionRequest, PermissionResult, PostToolUse, Stop, \
                      StopFailure, Interrupt) in ~/.kimi-code/config.toml:",
        command: MANUAL_HOOK_EXAMPLE,
        executable: false,
        link: None,
    }],
    post_install_notes: &["Restart Kimi Code to activate the hooks."],
});

fn config_path() -> io::Result<PathBuf> {
    kimi_home()
        .map(|home| home.join("config.toml"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine Kimi Code's home directory",
            )
        })
}

fn read_config() -> Option<String> {
    fs::read_to_string(config_path().ok()?).ok()
}

fn write_hooks() -> Result<(), PluginInstallError> {
    let path = config_path()?;
    let existing = match fs::read_to_string(&path) {
        Ok(config) => config,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let updated = with_warp_hooks(&existing, &hook_command()).map_err(|message| {
        PluginInstallError {
            log: format!("{}: {message}\n", path.display()),
            message,
        }
    })?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("toml.warp-tmp");
    fs::write(&temporary, updated)?;
    fs::rename(&temporary, &path)?;
    Ok(())
}

/// `config` with Warp's hook entries replaced by one per tracked event running `command`, and
/// everything else -- other hooks, comments, formatting -- as it was.
fn with_warp_hooks(config: &str, command: &str) -> Result<String, String> {
    let mut document = config
        .parse::<DocumentMut>()
        .map_err(|error| format!("Kimi Code's config.toml is not valid TOML: {error}"))?;
    let hooks = document
        .entry("hooks")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    let Some(hooks) = hooks.as_array_of_tables_mut() else {
        return Err(
            "Kimi Code's config.toml defines `hooks` other than as [[hooks]] tables".to_owned(),
        );
    };
    hooks.retain(|hook| !is_warp_hook(hook));
    for event in HOOK_EVENTS {
        let mut hook = Table::new();
        hook.insert("event", value(*event));
        hook.insert("command", value(command));
        hook.insert("timeout", value(HOOK_TIMEOUT_SECONDS));
        hooks.push(hook);
    }
    Ok(document.to_string())
}

fn is_warp_hook(hook: &Table) -> bool {
    hook.get("command")
        .and_then(Item::as_str)
        .is_some_and(|command| command.contains(COMMAND_MARKER))
}

fn warp_hooks(document: &DocumentMut) -> Vec<&Table> {
    document
        .get("hooks")
        .and_then(Item::as_array_of_tables)
        .map(|hooks| hooks.iter().filter(|hook| is_warp_hook(hook)).collect())
        .unwrap_or_default()
}

/// The version a Warp hook entry reports; `0.0.0` for one written before versioning.
fn hook_version(hook: &Table) -> &str {
    hook.get("command")
        .and_then(Item::as_str)
        .and_then(|command| {
            command
                .split_whitespace()
                .skip_while(|word| *word != "--hooks-version")
                .nth(1)
        })
        .unwrap_or("0.0.0")
}

/// The oldest version among Warp's hook entries, or `None` without any.
fn installed_hooks_version(config: &str) -> Option<String> {
    let document = config.parse::<DocumentMut>().ok()?;
    warp_hooks(&document)
        .into_iter()
        .map(hook_version)
        .min_by(|left, right| compare_versions(left, right))
        .map(str::to_owned)
}

/// Whether Warp's installed hooks are older than this build's or miss one of its events.
fn hooks_need_update(config: &str) -> bool {
    let Ok(document) = config.parse::<DocumentMut>() else {
        return false;
    };
    let hooks = warp_hooks(&document);
    if hooks.is_empty() {
        return false;
    }
    let outdated = hooks
        .iter()
        .any(|hook| compare_versions(hook_version(hook), HOOKS_VERSION).is_lt());
    let missing = HOOK_EVENTS.iter().any(|event| {
        !hooks
            .iter()
            .any(|hook| hook.get("event").and_then(Item::as_str) == Some(*event))
    });
    outdated || missing
}

#[cfg(test)]
#[path = "kimi_tests.rs"]
mod tests;
```
`plugin_manager/mod.rs`: add `pub(crate) mod kimi;` and `use kimi::KimiPluginManager;`; in `plugin_manager_for_with_shell`, before the unsupported group:
```rust
        CLIAgent::Kimi
            if FeatureFlag::KimiNotifications.is_enabled()
                && FeatureFlag::HOANotifications.is_enabled() =>
        {
            Some(Box::new(KimiPluginManager))
        }
```
(`CLIAgent::Kimi` also stays in the unsupported group, for when the guard fails.)

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run -p warp plugin_manager::kimi plugin_manager::tests::returns_manager_for_kimi plugin_manager::tests::returns_none_for_kimi`
Expected: PASS. If `installing_twice_changes_nothing` fails only on blank-line decor between tables, normalise the pushed tables' decor (`hook.decor_mut().set_prefix("\n")`) rather than weakening the test.

- [ ] **Step 6: Commit**

```bash
git add crates/warp_features/src/lib.rs app/src/terminal/cli_agent_sessions/plugin_manager/mod.rs app/src/terminal/cli_agent_sessions/plugin_manager/mod_tests.rs app/src/terminal/cli_agent_sessions/plugin_manager/kimi.rs app/src/terminal/cli_agent_sessions/plugin_manager/kimi_tests.rs
git commit -m "feat(cli-agent): install Warp hooks into Kimi Code's config.toml" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: The `@stozn/dsh-warp` Cordis plugin

Cordis facts (`deepseek-harness`, `@deepseek-ai/cordis` 4.0.5-alpha.1): a plugin module exports `name`, optional `inject`/`Config`, and `apply(ctx, config)`; a bundle package declares `"dsh": { "bundle": { "patch": "./cordis.patch.yml" } }`; events (`packages/core/agent/src/runtime-types.ts`): `'agent/created'({ agent, source: 'startup'|'resume'|'clear'|'compact', signal })` (serial; return `undefined`), `'agent/status'({ agent, status: 'idle'|'running' })`, `'agent/inbox/claimed'({ agent, message: UserMessage, turn })`, `'agent/turn-stopping'({ agent, turn, signal })`; `'tools/result'(exec, result)` is the observe-only twin of the `tools/post-execute` waterfall (`exec.name`, `exec.arguments`, `exec.agent?`); **`approval/asked` / `approval/decided` are session-log entries**, observed through `'session/event'(session, event)` with `event.type` and `event.data` (`{ id, toolName, callId?, reason? }` / `{ id, outcome }`). `agent.session.header` has `id`, `cwd?`, `version`, `origin?` (`'subagent'` for delegated agents). dsh exposes no transcript path; the plugin computes it with dsh's own `projectKey` and `encodeSegment` (`session-persistence-jsonl/src/format.ts:199-245`).

Deviations from the spec table, both forced by the source: `tools/result` instead of `tools/post-execute` (no `next()` to forward), and `session/event` filtered on `approval/asked` / `approval/decided` instead of subscribing to those names. Subagent sessions are ignored.

**Files:**
- Create: `app/resources/dsh-warp/package.json`, `app/resources/dsh-warp/tsconfig.json`, `app/resources/dsh-warp/cordis.patch.yml`, `app/resources/dsh-warp/src/protocol.ts`, `app/resources/dsh-warp/src/protocol.test.ts`, `app/resources/dsh-warp/src/index.ts`, `app/resources/dsh-warp/src/index.test.ts`, `app/resources/dsh-warp/README.md`
- Modify: `.gitignore` (append `app/resources/dsh-warp/node_modules/` and `app/resources/dsh-warp/dist/`)

**Interfaces:**
- Consumes: Task 9 `agent-event --format protocol`; Task 7 `WARP_AGENT_EVENT_BIN`.
- Produces: npm package `@stozn/dsh-warp@0.1.0` (reports `plugin_version: "0.1.0"`, `agent: "dsh"`), consumed by Task 12's installer.

- [ ] **Step 1: Write the failing test**

`app/resources/dsh-warp/src/protocol.test.ts`:
```ts
import assert from 'node:assert/strict'
import { join } from 'node:path'
import { test } from 'node:test'

import { dshHome, encodeSegment, notification, projectKey, textOf, transcriptPath } from './protocol.ts'

test('projectKey matches dsh', () => {
  // dsh's own vectors (session-persistence-jsonl/tests/jsonl.spec.ts:487-493), then two by hand.
  assert.equal(projectKey('/Users/qyj/work/deepseek-harness'), '--Users-qyj-work-deepseek-harness--')
  assert.equal(projectKey('/a/b-c'), projectKey('/a-b/c'))
  assert.equal(projectKey('C:\\work\\agent'), '--C-work-agent--')
  assert.equal(projectKey('/开发/~agent'), '--~5F00~53D1-~007Eagent--')
  assert.equal(projectKey('/'), '--root--')
  assert.equal(projectKey('/' + 'x'.repeat(1_000)).length, 255)
  assert.throws(() => projectKey(''), /empty project path/)
  assert.equal(projectKey('C:\\Users\\me\\proj'), '--C-Users-me-proj--')
  assert.equal(projectKey('/home/me/my project/'), '--home-me-my~0020project---')
})

test('encodeSegment matches dsh', () => {
  assert.equal(encodeSegment('..'), '~002E~002E')
  assert.equal(encodeSegment('.'), '~002E')
  assert.equal(encodeSegment('a/b'), 'a~002Fb')
  assert.equal(encodeSegment('/etc/passwd'), '~002Fetc~002Fpasswd')
  assert.equal(encodeSegment('a\u0000b'), 'a~0000b')
  assert.equal(encodeSegment('plain-ID_1.2'), 'plain-ID_1.2')
  assert.equal(encodeSegment('a~b'), 'a~007Eb')
  assert.equal(encodeSegment('session-0b6c1234-ffff-4a4a-8b8b-0123456789ab'), 'session-0b6c1234-ffff-4a4a-8b8b-0123456789ab')
})

test('dshHome honours DSH_HOME unless blank', () => {
  assert.equal(dshHome({ DSH_HOME: '/srv/dsh' }), '/srv/dsh')
  assert.notEqual(dshHome({ DSH_HOME: '   ' }), '   ')
  assert.ok(dshHome({}).endsWith('.dsh'))
})

test('transcriptPath is where dsh writes the session log', () => {
  const header = { id: 'session-1', cwd: 'C:\\Users\\me\\proj', version: 4 }
  assert.equal(
    transcriptPath(header, '/home/me/.dsh'),
    join('/home/me/.dsh', 'sessions', '--C-Users-me-proj--', 'session-1', 'session.v4.jsonl.zstd'),
  )
  assert.equal(transcriptPath({ id: 'session-1' }, '/home/me/.dsh'), undefined)
})

test('textOf joins text parts', () => {
  assert.equal(textOf([{ type: 'text', text: 'Fix the build' }, { type: 'image' }, { type: 'text', text: 'and the tests' }]), 'Fix the build\nand the tests')
  assert.equal(textOf('  plain  '), 'plain')
  assert.equal(textOf([{ type: 'image' }]), undefined)
  assert.equal(textOf(undefined), undefined)
})

test('notification is a protocol v1 event from dsh', () => {
  assert.deepEqual(notification('stop', { id: 'session-1' }, '0.1.0'), {
    v: 1,
    agent: 'dsh',
    event: 'stop',
    session_id: 'session-1',
    plugin_version: '0.1.0',
  })
  assert.deepEqual(notification('prompt_submit', { id: 'session-1' }, '0.1.0', { query: 'hi', summary: undefined }), {
    v: 1,
    agent: 'dsh',
    event: 'prompt_submit',
    session_id: 'session-1',
    plugin_version: '0.1.0',
    query: 'hi',
  })
})
```

- [ ] **Step 2: Package scaffolding and run the test to verify it fails**

`app/resources/dsh-warp/package.json`:
```json
{
  "name": "@stozn/dsh-warp",
  "version": "0.1.0",
  "description": "Reports DeepSeek Harness agent status to the WarpSpeed pane it runs in.",
  "license": "MIT",
  "type": "module",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "files": ["dist/index.js", "dist/index.d.ts", "dist/protocol.js", "dist/protocol.d.ts", "cordis.patch.yml", "README.md"],
  "dsh": { "bundle": { "patch": "./cordis.patch.yml" } },
  "scripts": {
    "build": "tsc -p tsconfig.json",
    "test": "node --test src/protocol.test.ts src/index.test.ts",
    "prepublishOnly": "npm test && npm run build"
  },
  "peerDependencies": { "@deepseek-ai/cordis": ">=4.0.5-alpha.1 <4.1.0" },
  "peerDependenciesMeta": { "@deepseek-ai/cordis": { "optional": true } },
  "devDependencies": { "@types/node": "^22.19.0", "typescript": "^5.9.0" },
  "engines": { "node": ">=22.19" },
  "publishConfig": { "registry": "https://git-dev.obachan.dev/api/packages/stozn/npm/" }
}
```
(The peer range records the Cordis version the plugin was written against; it is optional because the plugin uses only a structural `Context` type and imports nothing from Cordis at run time.)

`app/resources/dsh-warp/tsconfig.json`:
```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "rewriteRelativeImportExtensions": true,
    "erasableSyntaxOnly": true,
    "strict": true,
    "declaration": true,
    "rootDir": "src",
    "outDir": "dist",
    "types": ["node"],
    "skipLibCheck": true
  },
  "include": ["src/**/*.ts"],
  "exclude": ["src/**/*.test.ts"]
}
```
`app/resources/dsh-warp/cordis.patch.yml`:
```yaml
- insert:
    - id: dsh-warp
      name: '@stozn/dsh-warp'
```
Run:
```bash
cd app/resources/dsh-warp && npm install && node --test src/protocol.test.ts
```
Expected: FAIL — `Cannot find module …/src/protocol.ts`.

- [ ] **Step 3: Implement `protocol.ts`**

```ts
// The pure half of the plugin: what Warp's CLI agent protocol expects, and where dsh keeps a session.

import { homedir } from 'node:os'
import { join } from 'node:path'

export const AGENT = 'dsh'
export const PROTOCOL_VERSION = 1

/** A Warp CLI agent protocol v1 notification, as `warpspeed agent-event --format protocol` reads it. */
export interface Notification {
  v: number
  agent: string
  event: string
  session_id?: string
  cwd?: string
  query?: string
  transcript_path?: string
  summary?: string
  tool_name?: string
  tool_input?: unknown
  plugin_version?: string
}

/** The fields of dsh's `SessionHeader` this plugin reads. */
export interface SessionHeader {
  readonly id: string
  readonly cwd?: string
  readonly version?: number
  readonly origin?: string
}

/** dsh's `projectKey` (session-persistence-jsonl/src/format.ts:225-245), verbatim in behaviour. */
export function projectKey(cwd: string): string {
  if (cwd.length === 0) throw new Error('cannot encode an empty project path')
  let readable = ''
  let separatorRun = false
  for (let i = 0; i < cwd.length; i++) {
    const code = cwd.charCodeAt(i)
    const ch = String.fromCharCode(code)
    if (ch === '/' || ch === '\\' || ch === ':') {
      if (!separatorRun) readable += '-'
      separatorRun = true
    } else if (ch !== '~' && /^[A-Za-z0-9._-]$/.test(ch)) {
      readable += ch
      separatorRun = false
    } else {
      readable += '~' + code.toString(16).toUpperCase().padStart(4, '0')
      separatorRun = false
    }
  }
  const slug = readable.replace(/^-+/, '') || 'root'
  return `--${slug.slice(0, 251)}--`
}

/** dsh's `encodeSegment` (session-persistence-jsonl/src/format.ts:199-214), verbatim in behaviour. */
export function encodeSegment(raw: string): string {
  if (raw.length === 0) throw new Error('cannot encode an empty path segment')
  if (raw === '.') return '~002E'
  if (raw === '..') return '~002E~002E'
  let out = ''
  for (let i = 0; i < raw.length; i++) {
    const code = raw.charCodeAt(i)
    const ch = String.fromCharCode(code)
    if (ch !== '~' && /^[A-Za-z0-9._-]$/.test(ch)) {
      out += ch
    } else {
      out += '~' + code.toString(16).toUpperCase().padStart(4, '0')
    }
  }
  return out
}

/** dsh's data root: `$DSH_HOME` unless blank, else `~/.dsh`. */
export function dshHome(env: NodeJS.ProcessEnv = process.env): string {
  const configured = env.DSH_HOME?.trim()
  return configured ? configured : join(homedir(), '.dsh')
}

/**
 * Where the base bundle's JSONL persistence writes this session's current log. The file appears
 * on the first flush, and a session without a cwd lives elsewhere (`_no-cwd`), so it has none.
 */
export function transcriptPath(header: SessionHeader, home: string): string | undefined {
  if (!header.cwd) return undefined
  return join(home, 'sessions', projectKey(header.cwd), encodeSegment(header.id), `session.v${header.version ?? 4}.jsonl.zstd`)
}

/** The text of a message's content parts, joined; `undefined` when there is none. */
export function textOf(content: unknown): string | undefined {
  if (typeof content === 'string') return content.trim() || undefined
  if (!Array.isArray(content)) return undefined
  const text = content
    .filter((part): part is { type: 'text'; text: string } => part?.type === 'text' && typeof part.text === 'string')
    .map((part) => part.text)
    .join('\n')
    .trim()
  return text || undefined
}

/** A notification for `header`'s session. Fields of `extra` that are `undefined` are left out. */
export function notification(event: string, header: SessionHeader, pluginVersion: string, extra: Partial<Notification> = {}): Notification {
  const defined = Object.fromEntries(Object.entries(extra).filter(([, value]) => value !== undefined))
  return { v: PROTOCOL_VERSION, agent: AGENT, event, session_id: header.id, plugin_version: pluginVersion, ...defined }
}
```

- [ ] **Step 4: Run the protocol tests**

Run: `cd app/resources/dsh-warp && node --test src/protocol.test.ts`
Expected: all 6 tests pass.

- [ ] **Step 5: Write the failing event tests**

`app/resources/dsh-warp/src/index.test.ts` (one case per row of the spec's Cordis table, plus the filters):
```ts
import assert from 'node:assert/strict'
import { join } from 'node:path'
import { test } from 'node:test'

import { type Context, subscribe } from './index.ts'

type Listener = (...args: any[]) => unknown

/** A stand-in for Cordis' context that keeps the listeners, and what the plugin reports. */
function harness() {
  const listeners = new Map<string, Listener>()
  const ctx = {
    on: (name: string, listener: Listener) => {
      listeners.set(name, listener)
    },
  } as unknown as Context
  const reports: Array<{ id: string; event: string; extra: Record<string, unknown> }> = []
  subscribe(ctx, (header, event, extra = {}) => reports.push({ id: header.id, event, extra }), '/home/me/.dsh')
  const emit = (name: string, ...args: unknown[]) => listeners.get(name)!(...args)
  return { emit, reports }
}

const header = { id: 'session-1', cwd: '/work/repo', version: 4 }
const agent = { session: { header } }
const subagent = { session: { header: { ...header, id: 'session-2', origin: 'subagent' } } }

test('a started or resumed session reports session_start with its log', () => {
  const { emit, reports } = harness()
  assert.equal(emit('agent/created', { agent, source: 'startup' }), undefined)
  emit('agent/created', { agent, source: 'resume' })
  emit('agent/created', { agent, source: 'compact' })
  emit('agent/created', { agent: subagent, source: 'startup' })
  const transcript = join('/home/me/.dsh', 'sessions', '--work-repo--', 'session-1', 'session.v4.jsonl.zstd')
  assert.deepEqual(reports, [
    { id: 'session-1', event: 'session_start', extra: { cwd: '/work/repo', transcript_path: transcript } },
    { id: 'session-1', event: 'session_start', extra: { cwd: '/work/repo', transcript_path: transcript } },
  ])
})

test('prompts, tools, approvals, turn ends and idling are reported', () => {
  const { emit, reports } = harness()
  emit('agent/inbox/claimed', { agent, turn: 1, message: { content: [{ type: 'text', text: 'Fix it' }], source: { kind: 'user' } } })
  emit('agent/inbox/claimed', { agent, turn: 1, message: { content: [{ type: 'text', text: '<env>' }], source: { kind: 'runtime-context' } } })
  emit('tools/result', { name: 'bash', arguments: { command: 'ls' }, agent }, { isError: false })
  emit('tools/result', { name: 'bash', arguments: {} }, { isError: false })
  emit('session/event', agent.session, { type: 'approval/asked', data: { id: 'a1', toolName: 'bash', reason: 'Run ls?' } })
  emit('session/event', agent.session, { type: 'approval/asked', data: { id: 'a2', toolName: 'write' } })
  emit('session/event', agent.session, { type: 'approval/decided', data: { id: 'a1', outcome: 'allowed-once' } })
  emit('session/event', agent.session, { type: 'user/message', data: {} })
  emit('agent/turn-stopping', { agent, turn: 1 })
  emit('agent/status', { agent, status: 'running' })
  emit('agent/status', { agent, status: 'idle' })
  emit('agent/turn-stopping', { agent: subagent, turn: 1 })
  assert.deepEqual(
    reports.map(({ event, extra }) => ({ event, extra })),
    [
      { event: 'prompt_submit', extra: { query: 'Fix it' } },
      { event: 'tool_complete', extra: { tool_name: 'bash', tool_input: { command: 'ls' } } },
      { event: 'permission_request', extra: { tool_name: 'bash', summary: 'Run ls?' } },
      { event: 'permission_request', extra: { tool_name: 'write', summary: 'Approve write?' } },
      { event: 'permission_replied', extra: {} },
      { event: 'stop', extra: {} },
      { event: 'idle_prompt', extra: {} },
    ],
  )
})
```
Run: `cd app/resources/dsh-warp && npm test`
Expected: FAIL — `Cannot find module …/src/index.ts`.

- [ ] **Step 6: Implement `index.ts`**

```ts
// Cordis plugin: reports this DeepSeek Harness session's progress to the WarpSpeed pane it runs in,
// through `$WARP_AGENT_EVENT_BIN agent-event --format protocol`. Outside a WarpSpeed pane the
// variable is unset and the plugin does nothing.

import { type ChildProcess, spawn } from 'node:child_process'
import { readFileSync } from 'node:fs'

import { dshHome, notification, type Notification, type SessionHeader, textOf, transcriptPath } from './protocol.ts'

export const name = 'dsh-warp'

const VERSION: string = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).version

/** How long one delivery may take before the next is sent anyway; `agent-event` gives up after 2 s. */
const DELIVERY_TIMEOUT_MS = 3000

interface Session {
  readonly header: SessionHeader
}
interface Agent {
  readonly session: Session
}
interface UserMessage {
  readonly content?: unknown
  readonly source?: { readonly kind?: string }
}
interface ToolExecution {
  readonly name: string
  readonly arguments?: unknown
  readonly agent?: Agent
}
interface SessionEvent {
  readonly type: string
  readonly data?: { readonly toolName?: string; readonly reason?: string }
}

/** The part of Cordis' `Context` this plugin uses (`@deepseek-ai/cordis` 4.0.5-alpha.1). */
export interface Context {
  on(name: 'agent/created', listener: (payload: { agent: Agent; source: string }) => undefined): unknown
  on(name: 'agent/inbox/claimed', listener: (payload: { agent: Agent; message: UserMessage; turn: number }) => void): unknown
  on(name: 'tools/result', listener: (exec: ToolExecution, result: unknown) => void): unknown
  on(name: 'session/event', listener: (session: Session, event: SessionEvent) => void): unknown
  on(name: 'agent/turn-stopping', listener: (payload: { agent: Agent; turn: number }) => void): unknown
  on(name: 'agent/status', listener: (payload: { agent: Agent; status: 'idle' | 'running' }) => void): unknown
}

/** Reports one event of a session. */
export type Report = (header: SessionHeader, event: string, extra?: Partial<Notification>) => void

export function apply(ctx: Context): void {
  const binary = process.env.WARP_AGENT_EVENT_BIN
  if (!binary) return
  const send = serialSender(binary)
  subscribe(ctx, (header, event, extra) => send(notification(event, header, VERSION, extra)))
}

/** Turns the dsh events this plugin follows into `report` calls; `home` is dsh's data root. */
export function subscribe(ctx: Context, report: Report, home: string = dshHome()): void {
  const forSession: Report = (header, event, extra) => {
    // A subagent's session is not the one the pane shows.
    if (header.origin !== 'subagent') report(header, event, extra)
  }

  ctx.on('agent/created', ({ agent, source }) => {
    if (source === 'startup' || source === 'resume') {
      const header = agent.session.header
      forSession(header, 'session_start', { cwd: header.cwd, transcript_path: transcriptPath(header, home) })
    }
    return undefined
  })
  ctx.on('agent/inbox/claimed', ({ agent, message }) => {
    if (message.source?.kind === 'user') {
      forSession(agent.session.header, 'prompt_submit', { query: textOf(message.content) })
    }
  })
  ctx.on('tools/result', (exec) => {
    if (exec.agent) {
      forSession(exec.agent.session.header, 'tool_complete', { tool_name: exec.name, tool_input: exec.arguments })
    }
  })
  ctx.on('session/event', (session, event) => {
    if (event.type === 'approval/asked') {
      const toolName = event.data?.toolName
      forSession(session.header, 'permission_request', {
        tool_name: toolName,
        summary: event.data?.reason ?? (toolName ? `Approve ${toolName}?` : undefined),
      })
    } else if (event.type === 'approval/decided') {
      forSession(session.header, 'permission_replied')
    }
  })
  ctx.on('agent/turn-stopping', ({ agent }) => forSession(agent.session.header, 'stop'))
  ctx.on('agent/status', ({ agent, status }) => {
    if (status === 'idle') forSession(agent.session.header, 'idle_prompt')
  })
}

/** Sends notifications one at a time and in order, so a `stop` never overtakes its `prompt_submit`. */
function serialSender(binary: string): (event: Notification) => void {
  let queue: Promise<void> = Promise.resolve()
  return (event) => {
    queue = queue.then(() => deliver(binary, event))
  }
}

function deliver(binary: string, event: Notification): Promise<void> {
  return new Promise((resolve) => {
    let child: ChildProcess
    try {
      child = spawn(binary, ['agent-event', '--format', 'protocol'], { stdio: ['pipe', 'ignore', 'ignore'], windowsHide: true })
    } catch {
      resolve()
      return
    }
    const timer = setTimeout(() => {
      child.kill()
      finish()
    }, DELIVERY_TIMEOUT_MS)
    function finish() {
      clearTimeout(timer)
      resolve()
    }
    child.once('error', finish)
    child.once('exit', finish)
    child.stdin?.once('error', () => {})
    child.stdin?.end(JSON.stringify(event))
  })
}
```
`app/resources/dsh-warp/README.md`:
```markdown
# @stozn/dsh-warp

A DeepSeek Harness (Cordis) plugin that reports a session's progress — started, prompt, tool done, waiting for approval, turn done, idle — to the WarpSpeed pane it runs in, through `$WARP_AGENT_EVENT_BIN agent-event --format protocol`. Outside a WarpSpeed pane it does nothing.

WarpSpeed installs it from the pane footer (`dsh plugin --profile <profile> add @stozn/dsh-warp`, which needs pnpm). Source: `app/resources/dsh-warp/` in the WarpSpeed repository.

Build and test: `npm install && npm test && npm run build`. Publish to the Gitea registry with a token that has `write:package`: `npm publish`.
```
Append to the repo `.gitignore`:
```
app/resources/dsh-warp/node_modules/
app/resources/dsh-warp/dist/
```

- [ ] **Step 7: Run the tests and build**

Run: `cd app/resources/dsh-warp && npm test && npm run build && ls dist`
Expected: 8 tests pass; `dist/index.js dist/index.d.ts dist/protocol.js dist/protocol.d.ts`. Check that `dist/index.js` imports `./protocol.js` (not `.ts`): `grep -n "protocol" dist/index.js`.

- [ ] **Step 8: Commit**

```bash
git add app/resources/dsh-warp/package.json app/resources/dsh-warp/package-lock.json app/resources/dsh-warp/tsconfig.json app/resources/dsh-warp/cordis.patch.yml app/resources/dsh-warp/src/protocol.ts app/resources/dsh-warp/src/protocol.test.ts app/resources/dsh-warp/src/index.ts app/resources/dsh-warp/src/index.test.ts app/resources/dsh-warp/README.md .gitignore
git commit -m "feat(dsh-warp): add the Cordis plugin that reports DeepSeek Harness status to Warp" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 9: Publish (needs the user's Gitea token; ask before running)**

```bash
cd app/resources/dsh-warp
npm config set -- '//git-dev.obachan.dev/api/packages/stozn/npm/:_authToken' "<token with write:package>"
npm publish
npm view @stozn/dsh-warp@0.1.0 version --registry https://git-dev.obachan.dev/api/packages/stozn/npm/
```
Expected: `0.1.0`. Without a token, stop here and report; Task 12's unit tests do not need the package, Task 15 does.

---

### Task 12: The DeepSeek Harness plugin installer

`dsh plugin --profile <name> add <spec>` runs `pnpm` in `$DSH_HOME/profiles/<name>` (`deepseek-harness/apps/cli/src/args.ts:187-199`), so pnpm must be on PATH, and that directory's `.npmrc` must map `@stozn` to the Gitea registry.

**Files:**
- Create: `app/src/terminal/cli_agent_sessions/plugin_manager/dsh.rs`, `.../plugin_manager/dsh_tests.rs`
- Modify: `app/src/terminal/cli_agent_sessions/plugin_manager/mod.rs`, `.../plugin_manager/mod_tests.rs`

**Interfaces:**
- Consumes: Task 6 `dsh_home`; Task 10 `FeatureFlag::DshNotifications`; Task 11 package `@stozn/dsh-warp`.
- Produces: `plugin_manager_for_with_shell(CLIAgent::DeepSeekHarness, …)` → `DshPluginManager::new(shell_path, shell_type, path_env_var)` when `DshNotifications && HOANotifications`; private fns `profiles(home: &Path) -> Vec<String>`, `with_registry_line(existing: &str) -> Option<String>`, `installed_version(profile_dir: &Path) -> Option<String>`.

- [ ] **Step 1: Write the failing tests**

`plugin_manager/dsh_tests.rs`:
```rust
use std::fs;
use std::path::Path;

use super::{
    INSTRUCTIONS, MINIMUM_PLUGIN_VERSION, PACKAGE, REGISTRY_LINE, installed_version, profiles,
    with_registry_line,
};

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn installs_into_every_profile_or_the_headless_one() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(profiles(home.path()), vec!["headless".to_owned()]);

    for profile in ["web", "acp", ".cache", "with space"] {
        fs::create_dir_all(home.path().join("profiles").join(profile)).unwrap();
    }
    write(&home.path().join("profiles").join("notes.txt"), "");
    assert_eq!(profiles(home.path()), vec!["acp".to_owned(), "web".to_owned()]);
}

#[test]
fn registry_line_is_added_once() {
    assert_eq!(with_registry_line("").as_deref(), Some(format!("{REGISTRY_LINE}\n").as_str()));
    assert_eq!(
        with_registry_line("save-exact=true").as_deref(),
        Some(format!("save-exact=true\n{REGISTRY_LINE}\n").as_str())
    );
    assert_eq!(with_registry_line(&format!("{REGISTRY_LINE}\n")), None);
    assert_eq!(
        with_registry_line("  @stozn:registry=https://mirror.example/npm/\n"),
        None
    );
}

#[test]
fn the_manual_install_command_pins_the_installers_version() {
    let manual = INSTRUCTIONS
        .steps
        .iter()
        .map(|step| step.command)
        .find(|command| command.starts_with("dsh plugin"))
        .expect("the instructions show the install command");
    assert_eq!(
        manual,
        format!("dsh plugin --profile headless add {PACKAGE}@{MINIMUM_PLUGIN_VERSION}")
    );
}

#[test]
fn installed_version_reads_the_profiles_dependency() {
    let profile = tempfile::tempdir().unwrap();
    assert_eq!(installed_version(profile.path()), None);

    write(
        &profile.path().join("package.json"),
        r#"{"dependencies":{"left-pad":"1.0.0"}}"#,
    );
    assert_eq!(installed_version(profile.path()), None);

    write(
        &profile.path().join("package.json"),
        r#"{"dependencies":{"@stozn/dsh-warp":"^0.1.0"}}"#,
    );
    assert_eq!(installed_version(profile.path()).as_deref(), Some("0.0.0"));

    write(
        &profile
            .path()
            .join("node_modules")
            .join("@stozn")
            .join("dsh-warp")
            .join("package.json"),
        &format!(r#"{{"name":"@stozn/dsh-warp","version":"{MINIMUM_PLUGIN_VERSION}"}}"#),
    );
    assert_eq!(
        installed_version(profile.path()).as_deref(),
        Some(MINIMUM_PLUGIN_VERSION)
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p warp plugin_manager::dsh`
Expected: compile error.

- [ ] **Step 3: Implement**

`plugin_manager/dsh.rs`:
```rust
//! Installs `@stozn/dsh-warp`, the Cordis plugin that reports DeepSeek Harness sessions to Warp,
//! into the user's dsh profiles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::{fs, io};

use async_trait::async_trait;
use serde_json::Value;

use super::{
    CliAgentPluginManager, PluginInstallError, PluginInstructionStep, PluginInstructions,
    compare_versions, run_cli_command_logged,
};
use crate::terminal::agent_session_paths::dsh_home;
use crate::terminal::model::session::LocalCommandExecutor;
use crate::terminal::shell::ShellType;

const PACKAGE: &str = "@stozn/dsh-warp";
/// The release the installer pins (`dsh plugin add @stozn/dsh-warp@<version>`), so a newer publish
/// is never pulled in unreviewed, and the oldest one the footer treats as up to date. Keep in
/// sync with app/resources/dsh-warp/package.json and bump them together.
const MINIMUM_PLUGIN_VERSION: &str = "0.1.0";
const REGISTRY_LINE: &str = "@stozn:registry=https://git-dev.obachan.dev/api/packages/stozn/npm/";
/// The profile `dsh --profile headless` uses, which is how dsh runs in a pane.
const DEFAULT_PROFILE: &str = "headless";
const PNPM_MISSING: &str = "Installing the DeepSeek Harness plugin needs pnpm on PATH. Install it \
                            with `npm install -g pnpm`, then try again.";

pub(super) struct DshPluginManager {
    executor: LocalCommandExecutor,
    path_env_var: Option<String>,
}

impl DshPluginManager {
    pub(super) fn new(
        shell_path: Option<PathBuf>,
        shell_type: Option<ShellType>,
        path_env_var: Option<String>,
    ) -> Self {
        Self {
            executor: LocalCommandExecutor::new(shell_path, shell_type.unwrap_or(ShellType::Bash)),
            path_env_var,
        }
    }

    async fn run_logged(
        &self,
        cli: &str,
        args: &[&str],
        log: &mut String,
    ) -> Result<(), PluginInstallError> {
        let env_vars = self
            .path_env_var
            .as_deref()
            .map(|path| HashMap::from([("PATH".to_owned(), path.to_owned())]));
        run_cli_command_logged(cli, args, &self.executor, env_vars, log).await
    }

    async fn add_to_every_profile(&self) -> Result<(), PluginInstallError> {
        let home = dsh_home().ok_or_else(|| PluginInstallError {
            message: "could not determine DeepSeek Harness's home directory".to_owned(),
            log: String::new(),
        })?;
        let mut log = String::new();
        if let Err(error) = self.run_logged("pnpm", &["--version"], &mut log).await {
            return Err(PluginInstallError {
                message: PNPM_MISSING.to_owned(),
                log: error.log,
            });
        }
        let spec = format!("{PACKAGE}@{MINIMUM_PLUGIN_VERSION}");
        for profile in profiles(&home) {
            ensure_registry(&home.join("profiles").join(&profile))?;
            self.run_logged(
                "dsh",
                &["plugin", "--profile", &profile, "add", &spec],
                &mut log,
            )
            .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl CliAgentPluginManager for DshPluginManager {
    fn minimum_plugin_version(&self) -> &'static str {
        MINIMUM_PLUGIN_VERSION
    }

    fn can_auto_install(&self) -> bool {
        true
    }

    fn is_installed(&self) -> bool {
        !installed_versions().is_empty()
    }

    fn needs_update(&self) -> bool {
        installed_versions()
            .iter()
            .any(|version| compare_versions(version, MINIMUM_PLUGIN_VERSION).is_lt())
    }

    async fn install(&self) -> Result<(), PluginInstallError> {
        self.add_to_every_profile().await
    }

    async fn update(&self) -> Result<(), PluginInstallError> {
        self.add_to_every_profile().await
    }

    fn install_success_message(&self) -> &'static str {
        "Warp plugin installed. Start DeepSeek Harness again to activate."
    }

    fn update_success_message(&self) -> &'static str {
        "Warp plugin updated. Start DeepSeek Harness again to activate."
    }

    fn install_instructions(&self) -> &'static PluginInstructions {
        &INSTRUCTIONS
    }

    fn update_instructions(&self) -> &'static PluginInstructions {
        &INSTRUCTIONS
    }
}

static INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: "Install the Warp Plugin for DeepSeek Harness",
    subtitle: "The plugin reports the session's status to this pane. It is installed into each \
               dsh profile with pnpm.",
    steps: &[
        PluginInstructionStep {
            description: "Install pnpm if it is not on PATH",
            command: "npm install -g pnpm",
            executable: true,
            link: None,
        },
        PluginInstructionStep {
            description: "Let the profile's pnpm find the plugin: add this line to \
                          ~/.dsh/profiles/headless/.npmrc",
            command: REGISTRY_LINE,
            executable: false,
            link: None,
        },
        PluginInstructionStep {
            description: "Install the plugin into the profile (repeat for each profile you use)",
            command: "dsh plugin --profile headless add @stozn/dsh-warp@0.1.0",
            executable: true,
            link: None,
        },
    ],
    post_install_notes: &["Start DeepSeek Harness again to activate the plugin."],
});

/// The profiles to install into: each profile directory under `<home>/profiles`, or the headless
/// profile when there is none yet. Names that would need shell quoting are left out.
fn profiles(home: &Path) -> Vec<String> {
    let mut profiles = fs::read_dir(home.join("profiles"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| is_plain_profile_name(name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    profiles.sort();
    if profiles.is_empty() {
        profiles.push(DEFAULT_PROFILE.to_owned());
    }
    profiles
}

fn is_plain_profile_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
}

/// `.npmrc` contents that resolve `@stozn` packages from the Gitea registry, or `None` when
/// `existing` already names a registry for the scope.
fn with_registry_line(existing: &str) -> Option<String> {
    if existing
        .lines()
        .any(|line| line.trim_start().starts_with("@stozn:registry="))
    {
        return None;
    }
    let mut updated = existing.to_owned();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(REGISTRY_LINE);
    updated.push('\n');
    Some(updated)
}

fn ensure_registry(profile_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(profile_dir)?;
    let path = profile_dir.join(".npmrc");
    let existing = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if let Some(updated) = with_registry_line(&existing) {
        fs::write(path, updated)?;
    }
    Ok(())
}

/// The plugin version installed in a profile: `None` when the profile does not depend on it,
/// `0.0.0` when it does but the package is missing from `node_modules`.
fn installed_version(profile_dir: &Path) -> Option<String> {
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(profile_dir.join("package.json")).ok()?).ok()?;
    manifest["dependencies"].get(PACKAGE)?;
    let installed = fs::read_to_string(
        profile_dir
            .join("node_modules")
            .join("@stozn")
            .join("dsh-warp")
            .join("package.json"),
    )
    .ok()
    .and_then(|contents| serde_json::from_str::<Value>(&contents).ok());
    Some(
        installed
            .as_ref()
            .and_then(|package| package["version"].as_str())
            .unwrap_or("0.0.0")
            .to_owned(),
    )
}

/// The plugin version in each existing profile that has it.
fn installed_versions() -> Vec<String> {
    let Some(home) = dsh_home() else {
        return Vec::new();
    };
    profiles(&home)
        .into_iter()
        .filter_map(|profile| installed_version(&home.join("profiles").join(profile)))
        .collect()
}

#[cfg(test)]
#[path = "dsh_tests.rs"]
mod tests;
```
`plugin_manager/mod_tests.rs`: append
```rust
#[test]
fn returns_manager_for_deepseek_harness() {
    let _hoa_guard = crate::features::FeatureFlag::HOANotifications.override_enabled(true);
    let _dsh_guard = crate::features::FeatureFlag::DshNotifications.override_enabled(true);
    assert!(plugin_manager_for(CLIAgent::DeepSeekHarness).is_some());
}
```
`plugin_manager/mod.rs`: add `pub(crate) mod dsh;`, `use dsh::DshPluginManager;`, and before the unsupported group:
```rust
        CLIAgent::DeepSeekHarness
            if FeatureFlag::DshNotifications.is_enabled()
                && FeatureFlag::HOANotifications.is_enabled() =>
        {
            Some(Box::new(DshPluginManager::new(
                shell_path,
                shell_type,
                path_env_var,
            )))
        }
```

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p warp plugin_manager`
Expected: PASS, including Task 10's `returns_manager_for_kimi` and the `returns_manager_for_deepseek_harness` test added in Step 3.

- [ ] **Step 5: Commit**

```bash
git add app/src/terminal/cli_agent_sessions/plugin_manager/mod.rs app/src/terminal/cli_agent_sessions/plugin_manager/mod_tests.rs app/src/terminal/cli_agent_sessions/plugin_manager/dsh.rs app/src/terminal/cli_agent_sessions/plugin_manager/dsh_tests.rs
git commit -m "feat(cli-agent): install the Warp plugin into DeepSeek Harness profiles" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Format, lint, test, push

- [ ] **Step 1: Format**

Run (Git Bash): `./script/format`
Expected: exits 0; `git diff --stat` shows only formatting of files this plan touched. Stage and commit any changes as `style: format Kimi and DeepSeek Harness changes` (explicit paths).

- [ ] **Step 2: Clippy on the touched crates**

Run:
```bash
cargo clippy -p warp -p warp_cli -p warp_core -p warp_features -p input_classifier --all-targets --tests -- -D warnings
```
Expected: no warnings. Fix in place (no `#[allow]` unless the existing code does the same for the same reason).

- [ ] **Step 3: Tests**

Run:
```bash
cargo nextest run --no-fail-fast -p warp_cli -p warp_core -p warp_features -p input_classifier
cargo nextest run --no-fail-fast -p warp
cd app/resources/dsh-warp && npm test
```
Expected: all pass except the known `StoznAuthManager` failures that also fail on `dev` (compare the failing names against a `dev` run if there are more than ~22).

- [ ] **Step 4: Push**

```bash
git push gitea feat/kimi-dsh-agents
git push gitlab feat/kimi-dsh-agents
```
Never push to `origin`.

---

### Task 14: Real-machine verification on Windows (spec A·6)

Runs against an installed WarpSpeed in an **isolated profile instance**, so it can be quit and restarted without touching windows that hold live sessions.

- [ ] **Step 1: Prerequisites**

```powershell
npm install -g @moonshot-ai/kimi-code@2.1.1 @deepseek-ai/dsh@0.2.1-alpha.1 pnpm
kimi --version; dsh --version; pnpm --version
```
Kimi must be logged in / configured with a model (`kimi` once, by the user), and dsh must have a provider configured for its headless profile. Task 11 Step 9 must have published `@stozn/dsh-warp@0.1.0`.

- [ ] **Step 2: Install WarpSpeed and open the isolated instance**

```powershell
powershell -ExecutionPolicy Bypass -File T:\projects\dev-ops\warp-kimi-dsh\script\windows\install-warpspeed.ps1
$root = "$env:LOCALAPPDATA\Programs\WarpSpeed"
$exe = Join-Path (Join-Path $root (Get-Content "$root\current.txt")) 'warpspeed.exe'
$env:WARPSPEED_PROFILE = "kimi-e2e"
Start-Process $exe
```
(`install-warpspeed.ps1` installs into `<root>\<version>\` and names the current version in `current.txt`.) Do not stop or restart any other WarpSpeed process. Run every `warpspeed sessions …` below as `& $exe sessions …` with `$env:WARPSPEED_PROFILE = "kimi-e2e"` set, so it addresses this instance.

- [ ] **Step 3: Pane environment and the silent command**

In a pane of the `kimi-e2e` instance:
```powershell
$env:WARP_AGENT_EVENT_TOKEN; $env:WARP_AGENT_EVENT_BIN
Measure-Command { '{"hook_event_name":"Stop","session_id":"session_x"}' | & $env:WARP_AGENT_EVENT_BIN agent-event --format kimi-hook | Out-String | Set-Variable out }; "stdout=[$out] exit=$LASTEXITCODE"
```
Expected: a 32-hex token, the installed `warpspeed.exe` path; `stdout=[]`, exit 0, under 3 s.

- [ ] **Step 4: Kimi Code in a pane**

1. Run `kimi`. Expected: Kimi icon and footer; the footer offers "Install" (Warp hooks).
2. Click Install. Expected: toast "Warp hooks installed. Restart Kimi Code to activate."; `~\.kimi-code\config.toml` has 8 `[[hooks]]` entries with `command = "\"%WARP_AGENT_EVENT_BIN%\" agent-event --format kimi-hook --hooks-version 1.0.0"` and `timeout = 5`; any existing settings unchanged.
3. `/exit`, run `kimi` again, send "list the files here, then run `git status`". Expected: status working → needs approval (approval for the shell command, summary shown) → working after approving → done, with a "Notification from Kimi Code" when the window is not focused.
4. `warpspeed sessions panes --json` (same `WARPSPEED_PROFILE`). Expected: the pane has `"agent":"kimi"`, `agent_session_id` starting `session_`, `transcript_path` = the session directory under `~\.kimi-code\sessions\`.
5. Copy an image, paste it into the pane. Expected: Kimi attaches the image (Alt+V path).
6. Ctrl-G opens rich input; type `!git status`. Expected: shell-mode decorations; submitting runs it in Kimi's shell mode.
7. Detach and attach: `warpspeed sessions detach --session <session id from panes>` then `warpspeed sessions attach --session <id>`; send another prompt. Expected: status updates still arrive in the re-attached pane (the token survives adoption).
8. Restart: `warpspeed sessions quit` (this instance only), start it again with `WARPSPEED_PROFILE=kimi-e2e`, reopen the saved session if it does not restore automatically. Expected: the pane runs `kimi --session <same id>` and Kimi shows the previous conversation.

- [ ] **Step 5: DeepSeek Harness in a pane**

1. `dsh --profile headless "say hello"`. Expected: DeepSeek icon on the running block; the footer offers "Install" (Warp plugin).
2. Click Install. Expected: success toast; `~\.dsh\profiles\headless\.npmrc` contains the `@stozn:registry=` line; `~\.dsh\profiles\headless\node_modules\@stozn\dsh-warp\package.json` has version `0.1.0`. (Without pnpm: the toast names pnpm.)
3. `dsh --profile headless "list the files in this directory"`. Expected: status working → done; `warpspeed sessions panes --json` shows `"agent":"dsh"`, `agent_session_id` starting `session-`, `transcript_path` ending `session.v4.jsonl.zstd`, and that file exists once the run ends.
4. Open the agent session picker. Expected: the dsh session is listed with its first prompt as the title and no Resume action; the Kimi session from Step 4 is listed with Resume.

- [ ] **Step 6: Record the result**

Write the outcome (pass/fail per numbered check, with the session ids used) into the PR description for `feat/kimi-dsh-agents`; quit the `kimi-e2e` instance with `warpspeed sessions quit`.
