# @stozn/dsh-warp

A DeepSeek Harness (Cordis) plugin that reports a session's progress — started, prompt, tool done, waiting for approval, turn done, idle — to the WarpSpeed pane it runs in, through `$WARP_AGENT_EVENT_BIN agent-event --format protocol`. Outside a WarpSpeed pane it does nothing.

WarpSpeed installs it from the pane footer (`dsh plugin --profile <profile> add @stozn/dsh-warp`, which needs pnpm). Source: `app/resources/dsh-warp/` in the WarpSpeed repository.

Build and test: `npm install && npm test && npm run build`. Publish to the Gitea registry with a token that has `write:package`: `npm publish`.
