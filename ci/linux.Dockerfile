# Klipolo's Linux check-and-test image: Ubuntu 24.04 with Tauri's WebKitGTK build dependencies (for the app crate's
# tests), ALSA, Mesa lavapipe (Vulkan on the CPU, for the engine's GPU tests) and Rust 1.90 with clippy and rustfmt.
# The repository is mounted at run time; the image holds no source and no secrets.
#   docker build -f ci/linux.Dockerfile -t klipolo-ci ci
#   docker run --rm -v "$PWD":/src -w /src klipolo-ci scripts/ci-linux.sh
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
