#!/usr/bin/env sh
# Points this checkout's .build/ffmpeg-9.0 at a shared LGPL FFmpeg 9.0.2 build (scripts/build-ffmpeg.sh), so every
# checkout links the same prefixes without building its own.
#   scripts/link-ffmpeg.sh [build-dir]   default: the main checkout's .build/ffmpeg-9.0 (itself a link or a build)
# The same by hand: mkdir -p .build && ln -s <build-dir> .build/ffmpeg-9.0
# Safe to run again from any folder: a stale link is replaced, a real build directory is left alone, and nothing
# changes when <build-dir> doesn't exist.
set -eu
if [ $# -gt 0 ]; then
  want="$1"
else
  want="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/.build/ffmpeg-9.0"
fi
target="$(cd "$want" 2>/dev/null && pwd -P)" || { echo "No FFmpeg build at $want" >&2; exit 1; }
cd "$(git rev-parse --show-toplevel)"
link=.build/ffmpeg-9.0
if [ -d "$link" ] && [ ! -L "$link" ]; then
  echo "$link is a build directory of its own: left as it is"
elif [ "$(cd "$link" 2>/dev/null && pwd -P)" != "$target" ]; then
  mkdir -p .build
  ln -sfn "$target" "$link"
fi
found=0
for arch in x86_64 arm64; do
  if [ -f "$link/$arch/prefix/lib/pkgconfig/libavcodec.pc" ]; then
    echo "FFmpeg $arch prefix: $(cd "$link/$arch/prefix" && pwd -P)"
    found=1
  fi
done
[ $found -eq 1 ] || { echo "No FFmpeg 9.0.2 prefix under $link (build one with scripts/build-ffmpeg.sh)" >&2; exit 1; }
