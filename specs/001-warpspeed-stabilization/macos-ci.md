# macOS CI Options

## Recommendation: a macOS GitLab runner

The fork already builds in GitLab CI, but `.gitlab-ci.yml` is configured for a Linux container.
macOS builds need a separate macOS runner; a Linux Docker runner cannot produce/test this Cocoa app.
Do not change the Linux runner's default image/tags to pretend it can build macOS.

On an appropriately provisioned macOS runner:

1. Install the repository's pinned Rust toolchain, cargo-bundle, cargo-about, Xcode, and native build
   dependencies. Check out Git LFS resources. Cache registry/git downloads with deliberate limits.
2. Run formatting, the prescribed Clippy gates, targeted Rust tests, and macOS installer tests.
3. Run `./script/bundle --channel oss --debug --nouniversal --selfsign --no-dmg` for internal builds.
   Use an intentional release profile/signing flow for distributable builds, not this debug command.
4. Retain `target/debug/bundle/osx/WarpOss.app` in an archive that preserves bundle metadata and record
   the source commit, build flags, and test results. A downloaded build needs the appropriate signing
   and notarization strategy; ad-hoc/local development signing is not a public-distribution solution.
5. Never run the workstation installer against `/Applications` from a CI build job.

No new runner, signing secret, or remote job was provisioned by this task.

## Alternative: a self-hosted macOS Gitea Actions runner

Gitea Actions can build this app directly on macOS, without an Xcode wrapper project. Gitea supplies
runner software, not hosted Mac hardware: provide a dedicated Mac mini or another macOS machine.
Current upstream documentation names the binary `gitea-runner` (3.x); older installations use
`act_runner`. Select a runner release compatible with your Gitea instance and follow that release's
CLI/configuration documentation rather than mixing versions.

### Provision and register

1. Enable Actions for the Gitea repository. The source must be available there; pushing only to this
   fork's GitLab remote will not trigger Gitea jobs. Configure a deliberate mirror or a separate
   Gitea remote, with credentials kept outside the repository.
2. Create a dedicated, non-administrator macOS account with adequate disk space for Rust builds.
   Install Xcode and select its developer directory; complete first-launch setup and license
   acceptance. Provision Git LFS, the pinned Rust toolchain, and the dependencies described above.
   Install a Node.js version compatible with any JavaScript actions used by the workflow.
3. Download the runner's Darwin binary for the host architecture (`arm64` or `amd64`) from upstream
   releases, verify its published checksum, and install it on the account's PATH. Do not use a
   Linux runner binary or Docker for this native Cocoa build.
4. Create a private runner directory **outside the checkout** and register interactively:

   ```sh
   umask 077
   mkdir -p "$HOME/.local/share/gitea-runner"
   cd "$HOME/.local/share/gitea-runner"
   gitea-runner generate-config > config.yaml
   gitea-runner --config config.yaml register
   ```

   Supply the instance URL and a repository-scoped registration token from Gitea's Actions runner
   settings. Name the runner for the machine and use `macos-arm64:host` as its label on Apple Silicon
   (`macos-amd64:host` on Intel). Replace default Docker labels rather than advertising platforms the
   Mac cannot execute. Keep tokens and the generated `.runner` credentials out of Git and logs.
5. Set `runner.capacity: 1` in the generated configuration initially, then start the runner:

   ```sh
   gitea-runner --config config.yaml daemon
   ```

   Check that Gitea reports it online with the expected label. After a foreground smoke test, use
   `launchd` for persistence; configure an explicit working directory, HOME, and PATH including
   Cargo and Homebrew. The upstream binary-installation guide includes a launchd example. Run under
   the dedicated account, not root. A headless daemon is not sufficient verification for real-display
   GUI tests; those need an appropriate logged-in graphical session.

### Example workflow

The following is an **opt-in template**, not an installed or verified CI workflow. Save it as
`.gitea/workflows/macos.yml` in the Gitea repository after provisioning the runner. Confirm that the
instance supports manual `workflow_dispatch` triggers. On Intel, change `runs-on` to `macos-amd64`.
The workflow label omits the registration label's `:host` suffix.

```yaml
name: Native macOS build
on:
  workflow_dispatch:
jobs:
  build:
    runs-on: macos-arm64
    timeout-minutes: 90
    steps:
      - uses: https://github.com/actions/checkout@v4
        with:
          lfs: true
          persist-credentials: false
      - name: Verify and bundle
        shell: bash
        run: |
          set -euo pipefail
          export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
          ./script/presubmit
          python3 script/macos/test_install_warpspeed.py
          ./script/bundle --channel oss --debug --nouniversal --selfsign --no-dmg
          codesign --verify --deep --strict target/debug/bundle/osx/WarpOss.app
          mkdir -p ci-artifacts
          git rev-parse HEAD > ci-artifacts/source-commit.txt
          ditto -c -k --sequesterRsrc --keepParent \
            target/debug/bundle/osx/WarpOss.app ci-artifacts/WarpOss.zip
          shasum -a 256 ci-artifacts/WarpOss.zip > ci-artifacts/WarpOss.zip.sha256
```

Pin third-party actions to reviewed commit SHAs before enabling production use. This example leaves
artifacts on the runner; add an artifact-upload action explicitly supported by your Gitea version or
copy them to controlled storage before workspace cleanup. Do not assume GitHub's artifact service or
all versions of `actions/upload-artifact` work with Gitea. Set retention limits for artifacts, caches,
and logs. Never use the workstation installer to replace `/Applications/warpseed.app` in CI.

`presubmit` deliberately gates packaging. The failures recorded in [progress.md](progress.md) must be
resolved, and shell/SSH integration prerequisites provisioned, before expecting this template to
produce an artifact. No runner registration, Gitea mirror, workflow deployment, or remote verification
has been performed by adding these instructions.

### Security and signing

Host jobs execute directly as the runner account: `:host` is **not a sandbox**. Limit the runner to
trusted repositories and trusted workflow revisions; do not run unreviewed pull-request/fork code
with host access or signing credentials. Keep personal files, production credentials, and developer
login keychains off the runner. Use separate disposable machines for untrusted work.

The example produces a development-signed internal build. Public distribution still needs a separate
Developer ID signing/notarization workflow with a dedicated keychain and restricted secret access.
Do not put registration tokens, certificates, passwords, or notarization credentials in YAML.

References:

- [Gitea runner overview](https://docs.gitea.com/runner/)
- [Binary installation and launchd](https://docs.gitea.com/runner/installation/binary/)
- [Runner registration](https://docs.gitea.com/runner/registration/)
- [Runner labels and host execution](https://docs.gitea.com/runner/labels/)

## Xcode Cloud: possible, but requires an Xcode product wrapper

Apple's setup flow analyzes an Xcode project/workspace and its app/framework products. The product
needs a scheme with the archive action enabled and an App Store Connect app record. This Cargo
workspace is not directly such an Xcode product.

A feasible integration would add a small macOS Xcode app target/shared scheme whose build scripts
invoke the Rust build and assemble the native executable/resources into the target bundle. Custom
Xcode Cloud scripts would install Rust/dependencies and prepare resources. The target's bundle ID,
signing, archive, entitlements, and App Store Connect identity must be configured deliberately.
Repository/dependency access must also be granted to Xcode Cloud.

This is a separate packaging/CI project, not a switch that can be enabled for the current Cargo
build. First prove the wrapper target archives locally, then configure the first Cloud workflow in
Xcode and validate the resulting artifact. Keep this optional until the direct macOS build is stable.

References:

- [Configuring your first Xcode Cloud workflow](https://developer.apple.com/documentation/xcode/configuring-your-first-xcode-cloud-workflow)
- [Writing custom build scripts](https://developer.apple.com/documentation/xcode/writing-custom-build-scripts)
