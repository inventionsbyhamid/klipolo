#!/usr/bin/env bash
# The portable half of the FFmpeg hygiene check (every OS): configure's license must be LGPL 2.1 or later, and GPL,
# nonfree and version3 must be off. scripts/verify-ffmpeg-prefix.sh (macOS) and scripts/build-ffmpeg-linux.sh run it.
#   verify-ffmpeg-license.sh <log-dir>
# <log-dir> holds configure.log and config.h (or src/ffmpeg-*/config.h).
set -euo pipefail
D="$1"; rc=0
grep -E "^License:" "$D/configure.log"
grep -q "^License: LGPL version 2.1 or later" "$D/configure.log" || { echo "FAIL: license"; rc=1; }
cfg=""   # config.h from the source tree, or the copy kept next to configure.log once the tree is deleted
for c in "$D"/src/ffmpeg-*/config.h "$D"/config.h; do [ -f "$c" ] && { cfg="$c"; break; }; done
[ -n "$cfg" ] || { echo "FAIL: no config.h"; exit 1; }
grep -E "define (CONFIG_GPL|CONFIG_NONFREE|CONFIG_VERSION3) " "$cfg"
grep -qE "define (CONFIG_GPL|CONFIG_NONFREE|CONFIG_VERSION3) 1" "$cfg" && { echo "FAIL: gpl/nonfree/version3 enabled"; rc=1; }
grep -m1 FFMPEG_CONFIGURATION "$cfg" | sed 's/#define FFMPEG_CONFIGURATION /configuration: /'
awk '/^External libraries:/{f=1} /^Libraries:/{f=0} f && NF' "$D/configure.log" | tr -s ' \n' ' '; echo
exit $rc
