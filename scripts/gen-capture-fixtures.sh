#!/usr/bin/env bash
# Test media for klipolo-capture (dev only; an ffmpeg CLI with libx264 and libx265, never linked into Klipolo): two
# seconds of 320x180 at 30 fps, a 1 s GOP, no B-frames, fragmented, so the reader's own walk extracts real packets for
# the writer tests; and the same frames as raw Annex B streams.
# Made once and committed in crates/klipolo-capture/tests/fixtures; tests never make them again. Nothing here touches a
# capture device. A file that already exists is kept (delete it to make it again), so a rerun never churns the
# committed media.
#   scripts/gen-capture-fixtures.sh
set -euo pipefail
cd "$(dirname "$0")/.."
out=crates/klipolo-capture/tests/fixtures
mkdir -p "$out"
gen() {
  local dest="$out/$1"
  shift
  [ -e "$dest" ] && return 0
  ffmpeg -n -loglevel error "$@" "$dest"
}
gen gop-h264.mp4 -f lavfi -i testsrc2=size=320x180:rate=30 -t 2 -c:v libx264 -profile:v high -g 30 -bf 0 \
  -movflags +frag_keyframe+empty_moov+default_base_moof
gen gop-hevc.mp4 -f lavfi -i testsrc2=size=320x180:rate=30 -t 2 -c:v libx265 -x265-params keyint=30:bframes=0 \
  -tag:v hvc1 -movflags +frag_keyframe+empty_moov+default_base_moof
# The same frames as a raw Annex B stream (start codes, the parameter sets before each keyframe), which is what FFmpeg's
# hardware encoders give on Windows and Linux (src/annexb.rs turns it into the AVCC our files carry).
gen gop-h264.annexb -i "$out/gop-h264.mp4" -c copy -bsf:v h264_mp4toannexb -f h264
gen gop-hevc.annexb -i "$out/gop-hevc.mp4" -c copy -bsf:v hevc_mp4toannexb -f hevc
