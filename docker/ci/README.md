# CI build image

The image used by the `clippy` and `build:warpspeed` jobs in `.gitlab-ci.yml`.
It is `rust:1-bookworm` with Warp's Linux build dependencies already installed.

## Why it exists

Those jobs used to install their dependencies at job time:

```yaml
before_script:
  - apt-get update && apt-get install -y --no-install-recommends sudo
  - ./script/linux/install_build_deps
```

That cannot work on the self-hosted `obachan-staging-oss` runner. Its containers
do not run as root and have no sudo, so every `apt-get install` fails and the job
never reaches `cargo`. The dependencies have to be present before the job starts,
which means they have to be in the image.

It is also simply cheaper: the package installs and the protoc download ran on
every job, on a runner shared with granny, locke and ojiichan.

## Building and publishing

From the repository root:

```sh
docker build -f docker/ci/Dockerfile -t registry.gitlab.com/martin-wieser/warpspeed/ci:1.92.0 .
docker login registry.gitlab.com
docker push registry.gitlab.com/martin-wieser/warpspeed/ci:1.92.0
```

The tag tracks the `channel` in `rust-toolchain.toml`. When that is bumped:
rebuild, republish under the new tag, and update `image:` in `.gitlab-ci.yml`.
Publishing needs a GitLab account with write access to the project's container
registry; a personal access token with the `write_registry` scope works as the
password for `docker login`.

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
* `fmt` deliberately stays on the stock `rust:1-bookworm`. rustfmt only parses
  source, so pulling this larger image for it would cost more than the job.
* `/etc/warpspeed-ci-image` records the protoc and rustc versions; the jobs
  `cat` it so a log identifies the image it ran on.
