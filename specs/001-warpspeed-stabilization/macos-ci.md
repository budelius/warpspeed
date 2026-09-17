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
