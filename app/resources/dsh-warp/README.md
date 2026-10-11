# @stozn/dsh-warp

A DeepSeek Harness (Cordis) plugin that reports a session's progress — started, prompt, tool done, waiting for approval, turn done, idle — to the WarpSpeed pane it runs in, through `$WARP_AGENT_EVENT_BIN agent-event --format protocol`. Outside a WarpSpeed pane it does nothing.

WarpSpeed embeds the packed `stozn-dsh-warp-<version>.tgz` in its binary. The pane footer's Install button writes it to `<dsh home>/warp/` and runs `dsh plugin --profile <profile> add <that path>` for each profile (which needs pnpm), so no registry is involved. Source: `app/resources/dsh-warp/` in the WarpSpeed repository.

Build and test: `npm install && npm test && npm run build`. To regenerate the bundled tarball after a change, bump `version` in `package.json`, run `npm ci && npm run build && npm pack --pack-destination .`, commit the new `.tgz` (removing the old one), and bump the version in `plugin_version!` in `app/src/terminal/cli_agent_sessions/plugin_manager/dsh.rs` to match.
