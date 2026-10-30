#!/usr/bin/env bash
# Builds the LGPL FFmpeg 9.0.2 that Klipolo links and ships in Contents/Frameworks, one prefix per architecture: the
# Mac's own one natively, the other (Apple Silicon on an Intel Mac, or the reverse) as a cross build. Linux:
# scripts/build-ffmpeg-linux.sh (same flags, see ffmpeg-common.sh).
#   scripts/build-ffmpeg.sh [--arch x86_64|arm64] [build-dir]   defaults: this Mac's arch, .build/ffmpeg-9.0
# Layout: <build-dir>/dl (verified tarball, shared by both arches), <build-dir>/<arch>/logs (configure/make/install
# logs + config.h), <build-dir>/<arch>/prefix (lib, include). A symlinked build dir is followed, so checkouts can share
# one build: scripts/link-ffmpeg.sh, or `mkdir -p .build && ln -s <shared>/ffmpeg-9.0 .build/ffmpeg-9.0`.
# Builds into prefix.new, verifies it, then swaps it in with a rename (whoever waits for prefix sees all of it or none):
# the previous prefix stays as prefix.old (and its logs as logs.old) until you delete it, so delete it only after the
# engine tests pass on the new one.
# The prefix is neutral (/opt/klipolo-ffmpeg, installed via DESTDIR) and the build runs out of tree with a relative
# source path, so no machine path ends up in the dylibs. Install names are @rpath/lib*.N.dylib.
# JOBS sets make's parallel jobs (default: every CPU). On a Mac in use: nice -n 10 env JOBS=4 scripts/build-ffmpeg.sh
set -euo pipefail
# VERSION, SHA256, KEY_FPR, PREFIX, the shared flag list and the verified download (shared with the Linux build).
source "$(dirname "$0")/ffmpeg-common.sh"
ffmpeg_flags macos

host="$(uname -m)"
arch="$host"
if [ "${1:-}" = --arch ]; then arch="${2:?--arch takes x86_64 or arm64}"; shift 2; fi
case "$arch" in x86_64 | arm64) ;; *) echo "Unknown arch $arch: x86_64 or arm64" >&2; exit 1 ;; esac
# The other arch: configure can't run what it compiles, and clang (also the assembler and linker) targets that arch.
if [ "$arch" != "$host" ]; then
  FLAGS+=(--enable-cross-compile --arch="$arch" --target-os=darwin --cc="clang -arch $arch")
fi
# Every clang call (C, assembly and link) targets macOS 15.0, not the SDK's own version.
export MACOSX_DEPLOYMENT_TARGET=15.0

repo="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "${1:-$repo/.build/ffmpeg-9.0}"
B="$(cd "${1:-$repo/.build/ffmpeg-9.0}" && pwd -P)"
A="$B/$arch"
W="$A/work"
L="$A/logs.new"
G=""
# Every exit (success, failure or interrupt) removes the work tree and the throwaway GPG keyring.
cleanup() {
  if [ -n "$G" ]; then gpgconf --homedir "$G" --kill all 2>/dev/null || true; rm -rf "$G"; fi
  rm -rf "$W"
}
trap cleanup EXIT

# A build's work tree peaks at about 0.25 GB (source, objects, staged install); 2 GB leaves the rest of the Mac room.
free_kb=$(df -k "$B" | awk 'NR==2 {print $4}')
[ "$free_kb" -ge $((2 * 1024 * 1024)) ] || { echo "Need 2 GB free next to $B" >&2; exit 1; }

# 1. Source: reuse the tarball in dl/ or download it; SHA-256 always, GPG when gpg and the key file are here.
fetch_verified_source "$B"

# 2. Out-of-tree build. `src` beside the build folder makes configure use the relative source path ./src.
rm -rf "$W" "$L" "$A/prefix.new"
mkdir -p "$W/build" "$W/no-pkgconfig" "$L"
tar -xJf "$tarball" -C "$W" --no-same-owner
ln -s "../ffmpeg-$VERSION" "$W/build/src"
echo "Configuring $arch (log: $L/configure.log)"
(cd "$W/build" && PKG_CONFIG_LIBDIR="$W/no-pkgconfig" PKG_CONFIG_PATH='' src/configure --prefix="$PREFIX" "${FLAGS[@]}") \
  >"$L/configure.log" 2>&1 || { tail -20 "$L/configure.log" >&2; exit 1; }
echo "Building $arch (log: $L/make.log)"
make -C "$W/build" -j"${JOBS:-$(sysctl -n hw.ncpu)}" >"$L/make.log" 2>&1 || { tail -20 "$L/make.log" >&2; exit 1; }
make -C "$W/build" install DESTDIR="$W/stage" >"$L/install.log" 2>&1 || { tail -20 "$L/install.log" >&2; exit 1; }
cp "$W/build/config.h" "$L/config.h"
mv "$W/stage$PREFIX" "$A/prefix.new"
# pkg-config files find the prefix from their own location.
sed -i '' "s|$PREFIX|\${pcfiledir}/../..|g" "$A/prefix.new"/lib/pkgconfig/*.pc

# 3. Verify, then swap in.
"$repo/scripts/verify-ffmpeg-prefix.sh" "$L" "$A/prefix.new" "$arch"
rm -rf "$A/prefix.old" "$A/logs.old"
[ ! -e "$A/prefix" ] || mv "$A/prefix" "$A/prefix.old"
mv "$A/prefix.new" "$A/prefix"
[ ! -e "$A/logs" ] || mv "$A/logs" "$A/logs.old"
mv "$L" "$A/logs"
echo "FFmpeg $VERSION ($arch) installed at $A/prefix"
[ ! -e "$A/prefix.old" ] || echo "The previous prefix is at $A/prefix.old: delete it once the engine tests pass."
