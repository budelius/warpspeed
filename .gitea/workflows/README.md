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
