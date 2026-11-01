#!/usr/bin/env bash
# Dev-only synthetic media for engine tests (any ffmpeg CLI; nothing from it is linked or shipped).
# Never touches capture devices. `--bench` adds the benchmarks' media.
#
# Writes the set this version of the script makes: target/engine-fixtures/<version>/, the version being the script's
# POSIX `cksum` (its CRC and length), which the tests compute the same way to find it
# (crates/klipolo-engine/tests/support/fixture_set.rs). On a shared build server every checkout and gate may share
# target/engine-fixtures (one folder mounted into each run), and branches carry different versions of this script, so:
#   - each version has its own set, and a checkout never regenerates another branch's;
#   - one generator runs at a time (a lock on target/engine-fixtures/.lock), and a set is used once it is complete
#     (`.complete`, `.complete-bench`); run again, it does nothing;
#   - each file is written under a temporary name beside it and renamed into place, so a test reading it never sees
#     it half-written.
# Older sets stay where they are until someone removes them.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STORE="$ROOT/target/engine-fixtures"
VERSION=$(cksum < "${BASH_SOURCE[0]}" | awk '{ print $1 "-" $2 }')
OUT="$STORE/$VERSION"
BENCH=; [ "${1:-}" = "--bench" ] && BENCH=1
complete() { [ -e "$OUT/.complete" ] && { [ -z "$BENCH" ] || [ -e "$OUT/.complete-bench" ]; }; }
mkdir -p "$OUT"
if complete; then echo "engine fixtures: $OUT"; exit 0; fi
# One generator at a time over the shared folder: flock where there is one (Linux), else lockf (a Mac), held to the end.
if command -v flock >/dev/null 2>&1; then
  exec 9>"$STORE/.lock"
  flock 9
elif [ -z "${KLIPOLO_FIXTURES_LOCKED:-}" ] && command -v lockf >/dev/null 2>&1; then
  KLIPOLO_FIXTURES_LOCKED=1 exec lockf -k "$STORE/.lock" bash "${BASH_SOURCE[0]}" "$@"
fi
# Another generator may have finished this set while this one waited.
if complete; then echo "engine fixtures: $OUT"; exit 0; fi
# ffmpeg writes `.partial.<name>` (same extension, so the same container) and the file is renamed into place once it is
# whole. A file already in the set was made whole by this same script, so a stopped run resumes where it stopped (a
# partial file it left is overwritten).
ff() {
  local out="${!#}"
  [ -e "$out" ] && return 0
  local partial
  partial="$(dirname "$out")/.partial.$(basename "$out")"
  ffmpeg -hide_banner -loglevel error -y "${@:1:$#-1}" "$partial"
  mv -f "$partial" "$out"
}
# H.264 from VideoToolbox on a Mac (`-allow_sw 1`: the media engine when there is one, VideoToolbox's software encoder
# on VMs without one, such as hosted CI; same bytes where hardware exists); elsewhere (the Linux CI container) the
# distro CLI's libx264 at the same bitrates.
if [ "$(uname)" = Darwin ]; then H264=(-c:v h264_videotoolbox -allow_sw 1)
else H264=(-c:v libx264 -preset veryfast); fi
# HEVC Main 10 the same way (VideoToolbox on a Mac, libx265 elsewhere).
if [ "$(uname)" = Darwin ]; then HEVC10=(-c:v hevc_videotoolbox -allow_sw 1 -profile:v main10)
else HEVC10=(-c:v libx265 -preset veryfast -x265-params log-level=error); fi
BT709=(-colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv)
# h264_videotoolbox drops primaries/transfer unless the frames carry them, so tag the frames too.
TAG709="setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv"
# 1080p30 screen-like clip, 0.5 s GOP, 4 s, with a 1 kHz beep every second (A/V sync checks).
ff -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=4" \
   -f lavfi -i "sine=frequency=1000:sample_rate=48000:duration=4,volume='if(lt(mod(t,1),0.05),1,0)':eval=frame" \
   -vf "scale=out_color_matrix=bt709,$TAG709" "${H264[@]}" -b:v 8M -g 15 -pix_fmt yuv420p "${BT709[@]}" \
   -c:a aac -b:a 128k "$OUT/bars-30fps-0.5gop.mp4"
# 720p camera-like clip, no audio.
ff -f lavfi -i "testsrc=size=1280x720:rate=30:duration=4" -vf "hue=h=t*40,scale=out_color_matrix=bt709,$TAG709" \
   "${H264[@]}" -b:v 4M -g 15 -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/camera-720p.mp4"
# Flat saturated blue (#1E64DC), BT.709, for the colour test.
ff -f lavfi -i "color=c=0x1E64DC:size=1920x1080:rate=30:duration=2" -vf "scale=out_color_matrix=bt709,$TAG709" \
   "${H264[@]}" -b:v 4M -g 15 -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/solid-blue.mp4"
# Flat red, the same way, for the transition test (a dissolve from red into the blue above).
ff -f lavfi -i "color=c=red:size=1920x1080:rate=30:duration=2" -vf "scale=out_color_matrix=bt709,$TAG709" \
   "${H264[@]}" -b:v 4M -g 15 -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/solid-red.mp4"
# Portrait phone clip: stored 1280x720 (left half red, right half blue) with a display matrix that turns it 90°
# clockwise, like an iPhone (shown 720x1280: red on top, blue below).
ff -f lavfi -i "color=c=red:size=1280x720:rate=30:duration=1" \
   -vf "drawbox=x=640:y=0:w=640:h=720:color=blue:t=fill,scale=out_color_matrix=bt709,$TAG709" \
   "${H264[@]}" -b:v 2M -g 15 -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/phone-stored.mp4"
ff -display_rotation:v:0 -90 -i "$OUT/phone-stored.mp4" -c copy "$OUT/phone-portrait.mov"
# Mono 48 kHz tone for the mixer tests.
ff -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=4" -c:a aac -b:a 128k "$OUT/tone-440.m4a"
# The same as raw ADTS frames, which a test damages part-way (frames announcing no data: a run of damaged packets).
ff -i "$OUT/tone-440.m4a" -c copy -f adts "$OUT/tone-440.aac"
# Thumbnail scans: 130 s of 720p30 with a keyframe every second and a 440 Hz tone, so a file spans three 64-slot sprite
# chunks and its audio pass cuts them.
ff -f lavfi -i "testsrc2=size=1280x720:rate=30:duration=130" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=130" \
   -vf "scale=out_color_matrix=bt709,$TAG709" "${H264[@]}" -b:v 2M -g 30 -pix_fmt yuv420p "${BT709[@]}" \
   -c:a aac -b:a 128k "$OUT/scan-130s-720p.mp4"
# Long GOP (a keyframe every 80 s: at 0 and 80 s) over 140 s, so chunk 1 starts inside a GOP and chunk 2 has no
# keyframe of its own: each must still be written. Small, video only.
ff -f lavfi -i "testsrc2=size=320x180:rate=30:duration=140" -vf "scale=out_color_matrix=bt709,$TAG709" "${H264[@]}" \
   -b:v 300k -g 2400 -sc_threshold 0 -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/gop80-140s-180p.mp4"
# All-intra (MJPEG: every frame a keyframe, like ProRes): a scan seeks once per one-second slot instead of reading
# every frame.
ff -f lavfi -i "testsrc2=size=640x360:rate=30:duration=6" -c:v mjpeg -q:v 3 -pix_fmt yuvj420p -an \
   "$OUT/intra-6s-mjpeg.mov"
# The same in Matroska: its sample table at open holds only the packets read so far (the Cues are read at the first
# seek), so it must not be stored as the keyframe index.
ff -i "$OUT/intra-6s-mjpeg.mov" -c copy "$OUT/intra-6s-mjpeg.mkv"
# NTSC rate (29.97 fps, a keyframe every 15 frames: 0.5005 s apart, off the 1/100 s grid), 10 s of flat grey whose
# luma names the frame (16 + 8 × (frame mod 25)), so a seek test can tell which frame it got. Small, video only.
ff -f lavfi -i "color=c=gray:size=320x180:rate=30000/1001:duration=10" \
   -vf "geq=lum='16+8*mod(N\,25)':cb=128:cr=128,$TAG709" "${H264[@]}" -b:v 1M -g 15 -sc_threshold 0 \
   -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/ntsc-luma-10s.mp4"
# Film rate (23.976 fps, a keyframe every 2 s), 10 s whose luma names the frame, for editing copies at a source's exact
# rate: a copy must show the frame the original shows at every timeline frame. 720p: the copy's
# hardware encoder takes no smaller picture.
ff -f lavfi -i "color=c=gray:size=1280x720:rate=24000/1001:duration=10" \
   -vf "geq=lum='16+8*mod(N\,25)':cb=128:cr=128,$TAG709" "${H264[@]}" -b:v 1M -g 48 -sc_threshold 0 \
   -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/film-luma-10s.mp4"
# Like a phone or camera file: 1080p at 23.976 fps, a keyframe every 100 frames (at 0, 100 and 200),
# B-frames and AAC, about 12 s. The stalled-decoder tests block its reader at frame 210 (8.78 s) and check what export
# recovers. Its luma rises and falls in steps of 8 (16 + 8 × t, t = frame mod 50 up to 24, then back down), so frame
# 210 is 96 and its neighbours 88 and 104: no jump for VideoToolbox to take as a scene cut (it would add keyframes). Drawn
# small and scaled up (flat grey either way), so it takes seconds to make.
if [ "$(uname)" = Darwin ]; then GOP100=(-c:v h264_videotoolbox -allow_sw 1 -profile:v high -bf 2)
else GOP100=(-c:v libx264 -preset veryfast -profile:v high -bf 3 -keyint_min 100 -sc_threshold 0 -x264-params open-gop=0); fi
ff -f lavfi -i "color=c=gray:size=320x180:rate=24000/1001:duration=12" \
   -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=12" \
   -vf "geq=lum='16+8*if(lt(mod(N\,50)\,25)\,mod(N\,50)\,49-mod(N\,50))':cb=128:cr=128,scale=1920:1080:flags=neighbor,$TAG709" \
   "${GOP100[@]}" -b:v 4M -g 100 -pix_fmt yuv420p "${BT709[@]}" -c:a aac -b:a 128k -movflags +faststart -shortest \
   "$OUT/film-1080p-gop100-12s.mp4"
# The same at 30 fps for 2 s: frame starts that round down to whole µs (frame 1 at 33 333 µs) show their own frame.
ff -f lavfi -i "color=c=gray:size=320x180:rate=30:duration=2" \
   -vf "geq=lum='16+8*mod(N\,25)':cb=128:cr=128,$TAG709" "${H264[@]}" -b:v 1M -g 15 -sc_threshold 0 \
   -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/luma-30fps-2s.mp4"
# Editing copies: 20 s of 1080p30 with a keyframe every 10 s (at 0 and 10 s, like a screen recorder's long GOP) and a
# 440 Hz tone, so it needs a copy and its copy must drop the sound.
ff -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=20" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=20" \
   -vf "scale=out_color_matrix=bt709,$TAG709" "${H264[@]}" -b:v 6M -g 300 -sc_threshold 0 -pix_fmt yuv420p "${BT709[@]}" \
   -c:a aac -b:a 128k "$OUT/longgop-20s-1080p.mp4"
# The short-GOP clip in Matroska written as a live stream: no stated length, as MediaRecorder's WebM and unfinished
# recordings leave it, so the editing-copy probe measures it from its packets.
ff -i "$OUT/bars-30fps-0.5gop.mp4" -c copy -an -live 1 -f matroska "$OUT/bars-no-length.mkv"
# The same clip looped to 40 s (over 16 MiB, its last 16 MiB under 30 s): the probe measures the length from the file's
# tail and must still read the keyframes from its start.
ff -stream_loop 9 -i "$OUT/bars-30fps-0.5gop.mp4" -c copy -an -live 1 -f matroska "$OUT/bars-looped-no-length.mkv"
# 10-bit HEVC (Main 10, a phone's format): 2 s of flat grey whose luma names the frame (16 + 8 × (frame mod 25)), BT.709,
# small. The preview reads it through VideoToolbox as 8-bit.
ff -f lavfi -i "color=c=gray:size=640x360:rate=30:duration=2" \
   -vf "geq=lum='16+8*mod(N\,25)':cb=128:cr=128,format=yuv420p10le,$TAG709" "${HEVC10[@]}" -tag:v hvc1 -b:v 2M -g 15 \
   "${BT709[@]}" -an "$OUT/hevc10-luma-2s.mov"
# Sync's probe: 2 s of 64x64 with stereo AAC. lead-40ms.mp4's sound stream starts 40 ms after its picture, with
# a stated creation time. Its input offset is 40 ms plus the AAC encoder's 1024-sample priming: a stream that starts
# late keeps its priming (the edit list's media time stays 0), so its first timestamp, as the sound cache decodes it,
# is the offset minus the priming. aac-in-mov.mov has no offset: the priming rides the MOV edit list and is never a
# lead.
ff -f lavfi -i "testsrc2=size=64x64:rate=30:duration=2" -itsoffset 0.0613333 \
   -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=2" -ac 2 "${H264[@]}" -b:v 200k -g 15 -pix_fmt yuv420p \
   -c:a aac -b:a 128k -metadata creation_time=2026-10-16T10:00:00.000000Z "$OUT/lead-40ms.mp4"
ff -f lavfi -i "testsrc2=size=64x64:rate=30:duration=2" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=2" \
   -ac 2 "${H264[@]}" -b:v 200k -g 15 -pix_fmt yuv420p -c:a aac -b:a 128k "$OUT/aac-in-mov.mov"
# A phone file whose picture starts after its sound (Android's MPEG4Writer writes such an empty edit for
# the later track): sound from 0 and the first frame at 70 ms (exactly: a 1/30000 s time base, the frames passed through
# as they are, or ffmpeg 6 makes it 30000 fps), 2 s at 30 fps whose luma names the frame (16 + 8 × (frame mod 25)). No
# B-frames, so its sample table is its keyframe index.
ff -f lavfi -i "color=c=gray:size=320x180:rate=30:duration=2" \
   -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=2.1" \
   -vf "settb=1/30000,setpts=PTS+0.07/TB,geq=lum='16+8*mod(N\,25)':cb=128:cr=128,$TAG709" "${H264[@]}" -bf 0 \
   -fps_mode passthrough -enc_time_base:v 1:30000 -b:v 1M -g 15 -sc_threshold 0 -pix_fmt yuv420p "${BT709[@]}" \
   -c:a aac -b:a 128k "$OUT/video-starts-late.mp4"
: > "$OUT/.complete"
# Export benchmark (`--bench`): 60 s of noisy 1080p30 with peaky sound (quiet noise, loud beeps twice a
# second), so the loudness gain and the true-peak limiter both work. About a minute to make.
if [ -n "$BENCH" ]; then
  # VideoToolbox memory: 10 s of 4K testsrc2, as a phone's 10-bit HEVC (Main 10, HLG-tagged) and
  # as 8-bit H.264 for comparison, a keyframe every second.
  ff -f lavfi -i "testsrc2=size=3840x2160:rate=30:duration=10" -vf "format=yuv420p10le" "${HEVC10[@]}" -tag:v hvc1 \
     -b:v 40M -g 30 -colorspace bt2020nc -color_primaries bt2020 -color_trc arib-std-b67 -an "$OUT/uhd-hevc10-10s.mov"
  ff -f lavfi -i "testsrc2=size=3840x2160:rate=30:duration=10" -vf "scale=out_color_matrix=bt709,$TAG709" "${H264[@]}" \
     -b:v 40M -g 30 -pix_fmt yuv420p "${BT709[@]}" -an "$OUT/uhd-h264-10s.mp4"
  BEEP="0.03*(random(0)-0.5)+if(lt(mod(t\\,0.5)\\,0.03)\\,0.7*sin(2*PI*1000*t)\\,0)"
  ff -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=60,noise=alls=12:allf=t" \
     -f lavfi -i "aevalsrc='$BEEP|$BEEP':s=48000:d=60" \
     -vf "scale=out_color_matrix=bt709,$TAG709" "${H264[@]}" -b:v 10M -g 15 -pix_fmt yuv420p "${BT709[@]}" \
     -c:a aac -b:a 192k "$OUT/screen-60s-1080p30.mp4"
  # Limiter check: quiet noise with rare 10 ms bursts (about −22 LUFS, −6 dBTP: the loudness gain pushes the bursts
  # over the ceiling).
  BURST="0.004*(random(0)-0.5)+if(lt(mod(t\\,1)\\,0.01)\\,0.5*sin(2*PI*997*t)\\,0)"
  ff -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=10" -f lavfi -i "aevalsrc='$BURST|$BURST':s=48000:d=10" \
     -vf "scale=out_color_matrix=bt709,$TAG709" "${H264[@]}" -b:v 6M -g 15 -pix_fmt yuv420p "${BT709[@]}" \
     -c:a pcm_s16le "$OUT/peaks-10s-1080p30.mov"
  # 4K source: the landscape export becomes 3840 × 2160 and counts as two sessions.
  ff -f lavfi -i "testsrc2=size=3840x2160:rate=30:duration=3" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=3" \
     -vf "$TAG709" "${H264[@]}" -b:v 20M -g 15 -pix_fmt yuv420p "${BT709[@]}" -c:a aac "$OUT/uhd-3s.mp4"
  : > "$OUT/.complete-bench"
fi
ls -la "$OUT"
