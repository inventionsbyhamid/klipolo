#!/usr/bin/env bash
# Builds the LGPL FFmpeg 9.0.2 for Linux (the CI container, later the AppImage/Flatpak): the macOS flag list
# (scripts/ffmpeg-common.sh) without VideoToolbox, AudioToolbox and the macOS loader flags. Same layout as
# scripts/build-ffmpeg.sh:
#   scripts/build-ffmpeg-linux.sh [build-dir]  default .build/ffmpeg-9.0-linux
# <build-dir>/dl (verified tarball), logs (configure/make/install logs + config.h), prefix (lib, include). Builds into
# prefix.new, verifies it, then swaps it in (the previous prefix stays as prefix.old).
# The .so files get RUNPATH $ORIGIN (patchelf), so they find each other wherever the folder goes, and carry no
# machine path. Needs: build-essential, nasm, pkg-config, zlib1g-dev, patchelf, curl, xz-utils (gpg optional).
set -euo pipefail
source "$(dirname "$0")/ffmpeg-common.sh"
ffmpeg_flags linux

repo="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "${1:-$repo/.build/ffmpeg-9.0-linux}"
B="$(cd "${1:-$repo/.build/ffmpeg-9.0-linux}" && pwd -P)"
W="$B/work"
L="$B/logs.new"
G=""
cleanup() {
  if [ -n "$G" ]; then gpgconf --homedir "$G" --kill all 2>/dev/null || true; rm -rf "$G"; fi
  rm -rf "$W"
}
trap cleanup EXIT

free_kb=$(df -k "$B" | awk 'NR==2 {print $4}')
[ "$free_kb" -ge $((4 * 1024 * 1024)) ] || { echo "Need 4 GB free next to $B" >&2; exit 1; }

# 1. Source.
fetch_verified_source "$B"

# 2. Out-of-tree build with a relative source path (no machine path in the libraries).
rm -rf "$W" "$L" "$B/prefix.new"
mkdir -p "$W/build" "$W/no-pkgconfig" "$L"
tar -xJf "$tarball" -C "$W" --no-same-owner
ln -s "../ffmpeg-$VERSION" "$W/build/src"
echo "Configuring (log: $L/configure.log)"
(cd "$W/build" && PKG_CONFIG_LIBDIR="$W/no-pkgconfig" PKG_CONFIG_PATH='' src/configure --prefix="$PREFIX" "${FLAGS[@]}") \
  >"$L/configure.log" 2>&1 || { tail -20 "$L/configure.log" >&2; exit 1; }
echo "Building (log: $L/make.log)"
make -C "$W/build" -j"$(nproc)" >"$L/make.log" 2>&1 || { tail -20 "$L/make.log" >&2; exit 1; }
make -C "$W/build" install DESTDIR="$W/stage" >"$L/install.log" 2>&1 || { tail -20 "$L/install.log" >&2; exit 1; }
cp "$W/build/config.h" "$L/config.h"
mv "$W/stage$PREFIX" "$B/prefix.new"
sed -i "s|$PREFIX|\${pcfiledir}/../..|g" "$B/prefix.new"/lib/pkgconfig/*.pc
for f in "$B/prefix.new"/lib/lib*.so.*; do
  [ -L "$f" ] || patchelf --set-rpath '$ORIGIN' "$f"
done

# 3. Verify (the license half is shared with macOS; then RUNPATH and machine paths), then swap in.
rc=0
"$repo/scripts/verify-ffmpeg-license.sh" "$L" || rc=1
n=0
for f in "$B/prefix.new"/lib/lib*.so.*; do
  [ -L "$f" ] && continue
  n=$((n + 1))
  runpath=$(readelf -d "$f" | sed -n 's/.*(RUNPATH).*\[\(.*\)\]/\1/p')
  echo "-- $(basename "$f") RUNPATH=$runpath NEEDED: $(readelf -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | tr '\n' ' ')"
  [ "$runpath" = '$ORIGIN' ] || { echo "FAIL: RUNPATH is not \$ORIGIN"; rc=1; }
  ! readelf -d "$f" | grep -q '(RPATH)' || { echo "FAIL: an RPATH (not RUNPATH)"; rc=1; }
  paths=$(strings -a "$f" | LC_ALL=C grep -E '\.build/|/home/|/Users/' || true)
  [ -z "$paths" ] || { echo "FAIL: machine path(s) in $(basename "$f"): $(cut -c1-120 <<<"$paths")"; rc=1; }
done
[ "$n" -gt 0 ] || { echo "FAIL: no shared libraries in $B/prefix.new/lib"; rc=1; }
[ $rc -eq 0 ] || { echo "RESULT: FAIL"; exit 1; }
echo "RESULT: PASS (LGPL, RUNPATH \$ORIGIN, no machine paths; $n libraries)"
rm -rf "$B/prefix.old" "$B/logs.old"
[ ! -e "$B/prefix" ] || mv "$B/prefix" "$B/prefix.old"
mv "$B/prefix.new" "$B/prefix"
[ ! -e "$B/logs" ] || mv "$B/logs" "$B/logs.old"
mv "$L" "$B/logs"
echo "FFmpeg $VERSION installed at $B/prefix"
