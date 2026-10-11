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
