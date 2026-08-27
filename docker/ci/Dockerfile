# Build image for the WarpSpeed fork's GitLab CI.
#
# Why this exists
# ---------------
# `clippy` and `build:warpspeed` used to install their system dependencies at
# job time, via `script/linux/install_build_deps`. On the self-hosted
# `obachan-staging-oss` runner that cannot work: the runner's containers are
# not root and have no sudo, so every `apt-get install` fails and the job never
# reaches `cargo`. Baking the dependencies into the image removes the apt step
# from the job entirely -- nothing privileged happens at job time.
#
# It also removes the repeated cost: the package installs and the protoc
# download ran on every single job.
#
# Kept deliberately lean. `docker/linux-dev` is the interactive development
# container (sshd, gcloud, gh, a `dev` user); none of that belongs in CI.
#
# Build and publish (from the repository root):
#
#   docker build -f docker/ci/Dockerfile -t registry.gitlab.com/martin-wieser/warpspeed/ci:1.92.0 .
#   docker push registry.gitlab.com/martin-wieser/warpspeed/ci:1.92.0
#
# The tag tracks `rust-toolchain.toml`. Bump both together, and update the
# `image:` in .gitlab-ci.yml to match.

FROM rust:1-bookworm

# `script/linux/install_build_deps` sources `script/warp_sudo`, which shells out
# to `sudo` unconditionally. This image builds as root and has no sudo, so give
# it a shim that simply runs the command. Installing the real sudo package would
# work too, but there is no reason to carry a setuid binary in a CI image.
RUN printf '#!/bin/sh\nexec "$@"\n' > /usr/bin/sudo \
    && chmod 0755 /usr/bin/sudo

# `install_build_deps` unzips the protoc release but never installs `unzip`
# itself, and `rust:1-bookworm` does not ship one -- so the script would fail at
# that step. Install it up front rather than patching the script, which is
# shared with developer machines where unzip is usually already present.
RUN apt-get update \
    && apt-get install -y --no-install-recommends unzip ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /setup

# Copy only what the dependency scripts need. Keeping this narrow means the
# expensive layer below is not invalidated by unrelated source changes.
COPY script/ /setup/script/
COPY rust-toolchain.toml /setup/rust-toolchain.toml

# Strip carriage returns from the copied scripts.
#
# A Windows checkout of this repository has CRLF working-tree files (nothing in
# .gitattributes forces LF for `script/`), and `COPY` carries those bytes in
# verbatim. Linux then reads the shebang as `bash\r` and the script dies with
# "No such file or directory" -- exit 127, with no obvious cause. A CI checkout
# on Linux is LF and this is a no-op, so it costs nothing and makes the image
# buildable from a developer's Windows machine as well as from CI.
#
# rust-toolchain.toml gets the same treatment: the version is read out of it
# below, and a trailing CR turns "1.92.0" into a toolchain name rustup rejects
# with the carriage return invisible in the error message.
#
# Everything under script/ is text; there is nothing binary here to corrupt.
RUN find /setup/script -type f -exec sed -i 's/\r$//' {} + \
    && sed -i 's/\r$//' /setup/rust-toolchain.toml

# Install the system packages and protoc that Warp needs to compile, using the
# repository's own script so this image cannot drift from what developers run.
#
# WARP_SKIP_SUDO_PROMPT keeps `warp_sudo` from trying to read /dev/tty, which
# does not exist during a docker build.
ENV WARP_SKIP_SUDO_PROMPT=1
RUN apt-get update \
    && ./script/linux/install_build_deps \
    && rm -rf /var/lib/apt/lists/*

# Install the pinned toolchain and the components CI uses, so `fmt` and `clippy`
# do not each spend a `rustup component add` at job time.
RUN CHANNEL="$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)" \
    && rustup toolchain install "$CHANNEL" --profile minimal \
        --component rustfmt --component clippy \
    && rustup default "$CHANNEL"

# `install_build_deps` leaves protoc in /usr/local/bin, but the workspace build
# scripts read PROTOC explicitly.
ENV PROTOC=/usr/local/bin/protoc

WORKDIR /builds

# Record what this image is, so a job log identifies the image it ran on.
RUN protoc --version > /etc/warpspeed-ci-image \
    && rustc --version >> /etc/warpspeed-ci-image \
    && cat /etc/warpspeed-ci-image
