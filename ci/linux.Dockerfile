# Klipolo's Linux check-and-test image: Ubuntu 24.04 with Tauri's WebKitGTK build dependencies (for the app crate's
# tests), ALSA, Mesa lavapipe (Vulkan on the CPU, for the engine's GPU tests), Rust 1.90 with clippy and rustfmt, and
# Node 22 with pnpm (the page's typecheck and tests).
# The repository is mounted at run time; the image holds no source and no secrets.
#   docker build -f ci/linux.Dockerfile -t klipolo-ci ci
#   docker run --rm --name klipolo-gate --cpus 16 --memory 24g -v "$PWD":/src -w /src klipolo-ci scripts/ci-linux.sh
# The image runs as uid 1000, so the checkout must be owned by uid 1000. After an rsync, chown everything except
# target/ and .build/ (the container makes those itself):
#   chown 1000:1000 . && find . -mindepth 1 -maxdepth 1 ! -name target ! -name .build -exec chown -R 1000:1000 {} +
# The distro `ffmpeg` CLI only makes test fixtures (dev only, never linked: the engine links the LGPL build that
# scripts/build-ffmpeg-linux.sh makes, with nasm and patchelf).

FROM ubuntu:24.04
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential clang libclang-dev pkg-config nasm patchelf \
      ca-certificates curl git gnupg xz-utils binutils \
      zlib1g-dev libssl-dev libdbus-1-dev libxdo-dev \
      libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libayatana-appindicator3-dev librsvg2-dev \
      libasound2-dev \
      mesa-vulkan-drivers libvulkan1 libegl1 libgl1 \
      ffmpeg fonts-dejavu-core \
  && rm -rf /var/lib/apt/lists/*

# Rust as the image's own user (uid 1000), so files written to the mounted checkout aren't root's (and the engine's
# permission tests see a folder they can't read, which root always can).
USER ubuntu
ENV PATH=/home/ubuntu/.cargo/bin:$PATH \
    CARGO_INCREMENTAL=0
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain 1.90.0 -c clippy -c rustfmt
WORKDIR /src

# Node 22 and pnpm (the version apps/desktop/package.json names), for the page's typecheck and tests. Last, so the layers
# above stay cached. The tarball is checked against the SHA-256 nodejs.org publishes in SHASUMS256.txt.
USER root
ARG NODE_SHA256=680d3f30b24a7ff24b98db5e96f294c0070f8f9078df658da1bce1b9c9873c88
RUN curl -fsSL -o /tmp/node.tar.xz https://nodejs.org/dist/v22.21.1/node-v22.21.1-linux-x64.tar.xz \
  && echo "$NODE_SHA256  /tmp/node.tar.xz" | sha256sum -c - \
  && tar -xJ -f /tmp/node.tar.xz -C /usr/local --strip-components=1 \
      --exclude CHANGELOG.md --exclude README.md --exclude LICENSE \
  && rm /tmp/node.tar.xz \
  && npm install -g pnpm@11.15.1 \
  && npm cache clean --force
USER ubuntu
