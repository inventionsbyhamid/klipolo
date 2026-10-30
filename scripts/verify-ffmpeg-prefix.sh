#!/usr/bin/env bash
# Hygiene check of a macOS FFmpeg prefix before it ships in the app (scripts/build-ffmpeg.sh runs it). The license half
# is scripts/verify-ffmpeg-license.sh (shared with Linux); the lipo and otool half below is macOS only.
#   verify-ffmpeg-prefix.sh <log-dir> <prefix-dir> <arch>
# e.g. verify-ffmpeg-prefix.sh .build/ffmpeg-9.0/arm64/logs .build/ffmpeg-9.0/arm64/prefix arm64
# <log-dir> holds configure.log and config.h. Fails if configure's license is not LGPL, if GPL/nonfree/version3 are
# enabled, if a dylib isn't built for exactly <arch> or for macOS 15.0, if its install name isn't @rpath/…, if it loads
# anything outside @rpath/, /usr/lib/ and /System/ (e.g. Homebrew's /usr/local or /opt/homebrew), or if it carries a
# machine path (.build/ or /Users/) that would ship in the app.
set -euo pipefail
D="$1"; P="$2"; arch="$3"; rc=0
echo "== $D ($P, $arch)"
"$(dirname "$0")/verify-ffmpeg-license.sh" "$D" || rc=1
n=0
for f in "$P"/lib/lib*.dylib; do
  [ -L "$f" ] && continue
  n=$((n + 1))
  id=$(otool -D "$f" | tail -1)
  archs=$(lipo -archs "$f")
  minos=$(otool -l "$f" | awk '/LC_BUILD_VERSION/ {b = 1} b && $1 == "minos" {print $2; exit}')
  echo "-- $(basename "$f") [$archs, macOS $minos] id=$id"
  [ "$archs" = "$arch" ] || { echo "FAIL: built for $archs, not $arch"; rc=1; }
  [ "$minos" = 15.0 ] || { echo "FAIL: minimum macOS is $minos, not 15.0"; rc=1; }
  case "$id" in @rpath/*) ;; *) echo "FAIL: install name is not @rpath/"; rc=1 ;; esac
  bad=$(otool -L "$f" | tail -n +2 | awk '{print $1}' | grep -vE '^(@rpath/|/usr/lib/|/System/)' || true)
  [ -z "$bad" ] || { echo "FAIL: non-system load path(s): $bad"; rc=1; }
  otool -L "$f" | tail -n +2 | awk '{print "     " $1}'
  paths=$(strings -a "$f" | LC_ALL=C grep -E '\.build/|/Users/' || true)
  [ -z "$paths" ] || { echo "FAIL: machine path(s) in $(basename "$f"): $(cut -c1-120 <<<"$paths")"; rc=1; }
done
[ "$n" -gt 0 ] || { echo "FAIL: no dylibs in $P/lib"; rc=1; }
echo "dylibs checked: $n"
[ $rc -eq 0 ] || { echo "RESULT: FAIL"; exit 1; }
echo "RESULT: PASS (LGPL, $arch only, macOS 15.0, @rpath install names, only @rpath + /usr/lib + /System load paths," \
  "no machine paths)"
