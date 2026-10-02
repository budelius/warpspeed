# CI build image

The image used by the `fmt`, `clippy`, and `build` jobs in
`.gitea/workflows/warpspeed-ci.yml`. It is `rust:1-bookworm` with Warp's Linux
build dependencies already installed.

## Why it exists

Those jobs used to install their dependencies at job time:

```yaml
before_script:
  - apt-get update && apt-get install -y --no-install-recommends sudo
  - ./script/linux/install_build_deps
```

That could not work on the former non-root GitLab runner. Baking the dependencies
into the image also remains the right fit for the shared Gitea Actions runner pool:
it keeps privileged package setup out of build steps and avoids downloading the
same packages and protoc on every job.

## Building and publishing

From the repository root:

```sh
docker build -f docker/ci/Dockerfile -t harbor.pseekoo.io/stozn-warpspeed/ci:1.92.0 .
docker login harbor.pseekoo.io
docker push harbor.pseekoo.io/stozn-warpspeed/ci:1.92.0
```

The tag tracks the `channel` in `rust-toolchain.toml`. When that is bumped,
rebuild and republish under the new tag, then update both the tag and manifest
digest in `.gitea/workflows/warpspeed-ci.yml`. Publishing requires a Harbor robot
with push and pull access to the `stozn-warpspeed` project.

The workflow pins the Linux/amd64 manifest digest, so republishing a tag cannot
silently change the CI environment. The project is publicly readable because the
CI image was already anonymous upstream and must be pulled before a job can access
repository secrets; publishing remains authenticated.

Nothing rebuilds this image automatically. It changes only when the toolchain
or `script/linux/install_build_deps` changes, both of which are rare and both of
which want a human deciding when CI switches over.

## Notes

* The image installs its packages by running the repository's own
  `script/linux/install_build_deps`, so it cannot drift from what developers
  install on their own machines.
* `sudo` is a shim that execs its arguments, not the real package. The build
  runs as root, and a CI image has no reason to carry a setuid binary.
* `unzip` is installed explicitly. `install_build_deps` uses it to unpack the
  protoc release but never installs it, and `rust:1-bookworm` does not ship one.
* Carriage returns are stripped from the copied scripts. A Windows checkout has
  CRLF working-tree files — nothing in `.gitattributes` forces LF for `script/` —
  and `COPY` carries those bytes in, after which Linux reads the shebang as
  `bash\r` and the script dies with exit 127. On a Linux checkout the step does
  nothing.
* All jobs use the same Harbor image, removing runtime pulls from Docker Hub and
  the legacy GitLab registry.
* The Actions workflow installs only its checkout integration dependencies at
  job time: Node.js for the pinned Actions and Git LFS for the release checkout.
  Warp's compiler and native build dependencies remain pre-baked.
* Release jobs fetch LFS payloads from Gitea. The repository's complete LFS set
  was migrated and independently fetched from Gitea before the compatibility
  mirror override was removed.
* `/etc/warpspeed-ci-image` records the protoc and rustc versions; the jobs
  `cat` it so a log identifies the image it ran on.
