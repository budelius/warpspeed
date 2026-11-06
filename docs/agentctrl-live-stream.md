# AgentCtrl live output

Managed-session attach (including sessions opened in a GUI pane) and the selected Agent Sessions
transcript preview prefer `GET /api/agent-control/runs/{job}/events/stream`. They authenticate with
the stored STOZN/Zitadel sign-in and resume using the last applied opaque event cursor.

- `run_event` feeds the existing output renderer; replayed events are deduplicated.
- `gap` shows the existing history-gap notice and reconnects before the first retained event.
- `terminal` closes the stream after retained output is drained.
- `reauth` reconnects with a refreshed credential.
- Unsupported endpoints fall back to long polling. Transient stream failures use polling for
  30 seconds before trying SSE again. Credentials never appear in stream URLs.
- GUI previews retain the latest 12 display events, not an unbounded transcript. Selecting a
  different run or closing the last subscribing view stops further reads; an in-flight blocking
  read is released on the next event/keepalive or at the 60-second request deadline.

The endpoint default is unchanged. To opt a newly launched process into the staging public API:

```powershell
$env:STOZN_BASE_URL = "https://agents-dev.obachan.dev"
warpspeed agent-ctrl attach <job-id>
```

A running GUI uses the environment it inherited at startup; setting this variable in a later
shell does not change that GUI's selected endpoint. The dashboard endpoint also supports this
stream contract once the companion dashboard change is deployed. Do not put tokens in deep links.

## Staging rollout notes

- A new Caddy host needs a second route registration after its certificate has been issued.
- The staging Kubernetes API is reached at `https://100.64.0.2:6443`; retain the existing
  `--tls-server-name 10.96.0.1` override instead of editing the shared kubeconfig.
- APISIX's coarse flood guard is 3000 requests per 60 seconds **per observed Caddy pod address**,
  not one shared cluster-wide bucket. Staging's six Caddy pods have separate buckets. The API's
  per-user mutation limit remains the user-level limit.

## Verification

```sh
./script/format --check
cargo nextest run -p managed_agent_sessions --features tui
cargo clippy -p managed_agent_sessions --all-targets --features tui -- -D warnings
cargo clippy -p warp --lib --tests -- -D warnings
```

The staging acceptance check still requires an approved interactive run and user sign-ins:
start/open/input/stop, inspect owner-authored commits and model-spend attribution, verify archive,
and check that an `agent-user` cannot access another owner's runs. Local stream fixtures do not
establish those deployment or identity guarantees.
