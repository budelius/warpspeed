# WarpSpeed CI workflows

Gitea Actions reads workflows from the first directory in its `WORKFLOW_DIRS`
setting (default `.gitea/workflows`, then `.github/workflows`) that exists in the
triggering commit. Because this directory exists, the upstream workflows in
`.github/workflows/` are inert on any commit that contains it. They are kept
unmodified so upstream merges stay conflict-free.

Do not delete this directory. Any commit without
`.gitea/workflows/` (for example a branch cut before the Gitea cutover, or a raw
push of upstream `master`) falls back to `.github/workflows/`, and its
`ubuntu-latest` jobs will run on our runners.

`desktop-builds.yml` publishes rolling, unsigned desktop downloads only after macOS and Windows
build the same commit successfully:

- `https://git-dev.obachan.dev/stozn/warpspeed/releases/tag/continuous-dev`
- `https://git-dev.obachan.dev/stozn/warpspeed/releases/tag/continuous-main`

The macOS artifact is an Apple Silicon self-signed app bundle. The Windows artifact is an x64
unsigned installer. Each push to `dev` or `main` replaces that branch's downloads and includes
SHA-256 files plus source-commit markers.

## Windows runner operations

The `budbot-windows` runner provides the `windows-amd64` label. Its files, configuration, and
logs are under `T:\gitea-runner\warpspeed`, `T:\gitea-runner\warpspeed\config.yaml`, and
`T:\gitea-runner\warpspeed\runner.log`, respectively.

The runner is launched by the per-user Startup shortcut `Gitea WarpSpeed Windows Runner.lnk`.
It therefore starts only after the `budel` user signs in. It is not installed as a system service
or Scheduled Task because this account was denied permission to create the task. After a host
restart, Windows builds remain queued until that user signs in or starts the runner manually:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File T:\gitea-runner\warpspeed\start-runner.ps1
```

The launcher is idempotent and exits when this runner executable is already active. Check its
process and recent log output with:

```powershell
Get-Process gitea-runner
Get-Content T:\gitea-runner\warpspeed\runner.log -Tail 100
```
