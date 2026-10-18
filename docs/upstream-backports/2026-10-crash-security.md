# October 2026 crash and security backports

Base: `042609c416dcc86ce3b6a51e191934fd69ad89e0` (WarpSpeed dev).
Reviewed upstream: `b80b4e3e0d0492974da93f37e40751338eef5f9e`.

This batch deliberately preserves WarpSpeed's supervisor, session recovery, AgentCtrl, authentication,
and build/installation behavior. The upstream commits are recorded with cherry-pick provenance.

## Security and dependencies

| Upstream commit | Change |
| --- | --- |
| `a9520c97` | GraphQL tooling: js-yaml 4.3.2 |
| `1cc4edf6` | GraphQL tooling: brace-expansion 2.1.7 |
| `714c51f1` | GraphQL tooling: @fastify/busboy 3.2.2 |
| `c79019b5` | rustls 0.23.45, rustls-webpki 0.103.15, aws-lc-rs 1.18.1 / aws-lc-sys 0.45.0 |
| `114bacea` | rmcp / rmcp-macros 2.2.0 and associated API adaptations |
| `617c3a99` | Forward OAuth callback issuer through both application and loopback callbacks |

The Rust manifest also requires rustls >= 0.23.45 within the 0.23 series. GraphQL package changes
affect build/code-generation tooling; they do not replace the checked-in server schema.

rmcp 2.2 already exposes `handle_callback_with_issuer`. Its issuer-forwarding fix can therefore be
backported without taking the separate rmcp 3.5 / bounded agent-environment setup migration.
The application test module includes only the relevant upstream issuer regression, rather than
unrelated tests for newer reconnect machinery. Loopback tests additionally exercise decoded issuer
forwarding, callbacks without an issuer, and a full code exchange with issuer support advertised.

## Crash fixes

| Upstream commit | Change |
| --- | --- |
| `83e270f1` | Font-width fallback when glyph outline bounds are unavailable |
| `92a98662` | Empty streamed Agent document update |
| `a7326f8f` | Wide-character promotion at the terminal's right edge |
| `97037ec8` | Fallible view updates after window teardown |
| `3308af3a` | Undo-close cleanup of unavailable pane groups |
| `71cbee77` | Settings-error synchronization into unavailable panes |
| `5b9771ac` | Model-selector action targeting a closed window |
| `b231216a` | Reentrant inline-history Up/Down navigation |
| `58d94a53` | Reversed glyph-index bounds during smart selection |
| `83387d2e` | winit X11 attention-request panic fix |

The smart-selection regression uses the fork's existing byte-offset selection API.

## Deliberately deferred

- Presigned-storage client isolation (`1d32d02f`): requires a separate port and validation of the
  `redirect=false` download API contract; do not introduce newer harness persistence machinery just
  to apply it.
- MCP 3.5 and broader tool/environment timeout changes (`d5d23e61`).
- Claude transcript naming, WSL responsiveness, and harness shell-selection fixes.
- ACP/Pi cloud integration, native shell-completion migration, removal of completions v2, and
  upstream cloud team/billing features.

## Verification requirements

- Frozen GraphQL tooling install and installed-version/schema validation.
- Unit coverage for MCP OAuth, terminal grid/cells, font fallback, smart selection, editor buffers,
  fallible view updates, and application input/model-selector regressions.
- Existing supervisor, snapshot, and restart tests to detect interference with fork behavior.
- Repository formatting and all three Clippy configurations from `script/presubmit`.
- Native-platform verification for the TLS/native crypto and winit changes. Windows checks do not
  establish Linux X11 runtime behavior or macOS compatibility.

Results and unavailable verification environments should be reported with the change; this document
does not claim that every check has run or passed.
