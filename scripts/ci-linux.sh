#!/usr/bin/env bash
# The Linux gate: format, compile, lint and test the workspace's crates on Linux (with the engine's fixture tests),
# check that the page's committed bindings and the app's third-party notices are current, typecheck and test the page,
# and run the engine's GPU tests on Mesa lavapipe. Run inside ci/linux.Dockerfile's image, from the repository root:
#   docker build -f ci/linux.Dockerfile -t klipolo-ci ci
#   docker run --rm --name klipolo-gate --cpus 16 --memory 24g -v "$PWD":/src -w /src klipolo-ci scripts/ci-linux.sh
# The image runs as uid 1000, so the checkout must be owned by uid 1000. After an rsync, chown everything except
# target/ and .build/ (the container makes those itself):
#   chown 1000:1000 . && find . -mindepth 1 -maxdepth 1 ! -name target ! -name .build -exec chown -R 1000:1000 {} +
# Every step runs even when an earlier one fails; the summary lists each, and the exit code is non-zero if any failed.
# Nothing here opens a capture device or needs a secret. Builds go to $CARGO_TARGET_DIR, else target/ci-linux (never
# a Mac's target/debug).
set -uo pipefail
cd "$(dirname "$0")/.."
# The consent guard: a real capture panics before any OS call while this is set (klipolo-capture's `guard`).
export KLIPOLO_TEST="${KLIPOLO_TEST:-1}"

# The page's generated files as checked out, before any test step writes the bindings again ("bindings are current").
generated=$(mktemp -d)
cp -R apps/desktop/src/bindings apps/desktop/src/lib "$generated/"
export CARGO_INCREMENTAL=0 CARGO_TERM_COLOR=never
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target/ci-linux}"
results=()
run() {  # <name> <command…>
  local name="$1"; shift
  echo; echo "=== $name"
  local started=$SECONDS
  if "$@"; then results+=("PASS  $name ($((SECONDS - started)) s)")
  else results+=("FAIL  $name ($((SECONDS - started)) s)"); fi
}

# The LGPL FFmpeg 9.0.2 for Linux, built once and kept in .build (the macOS prefix's headers are the same).
prefix="$PWD/.build/ffmpeg-9.0-linux/prefix"
if [ ! -f "$prefix/lib/pkgconfig/libavcodec.pc" ]; then
  run "FFmpeg 9.0.2 LGPL for Linux" scripts/build-ffmpeg-linux.sh
fi
# pkg-config searches the prefix first, then the system's own directories (alsa-sys and the WebKitGTK stack find their
# packages there).
system_pc_path=$(pkg-config --variable pc_path pkg-config)
export FFMPEG_DIR="$prefix" KLIPOLO_FFMPEG_PREFIX="$prefix" PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig:$system_pc_path"
export LD_LIBRARY_PATH="$prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

# Test media (the distro ffmpeg CLI with libx264; dev only, never linked): the set this version of the script makes,
# target/engine-fixtures/<its cksum> (the tests find it the same way). Made once per version; a no-op when the set is
# complete.
run "engine fixtures" scripts/gen-engine-fixtures.sh

# Formatting, every crate and module (rustfmt walks every module whatever its cfg).
run "cargo fmt --all --check" cargo fmt --all --check
# The bundled pack (assets/library): what scripts/gen-library.py writes, the same bytes every run, CC0 and small.
run "bundled pack" scripts/gen-library.test.sh
run "cargo check --workspace --all-targets" cargo check --workspace --all-targets --locked
run "clippy model, cursor, platform, engine, fonts, speech-wire, meta, sam, analysis, chat, mcp, capture" \
  cargo clippy -p klipolo-model -p klipolo-cursor -p klipolo-platform -p klipolo-engine -p klipolo-fonts \
  -p klipolo-speech-wire -p klipolo-meta -p klipolo-sam -p klipolo-analysis -p klipolo-chat -p klipolo-mcp \
  -p klipolo-capture --features klipolo-capture/synthetic --all-targets --locked -- -D warnings
# The app crate: its non-GUI code (the engine commands, on the blank backend here; no window opens).
run "clippy app" cargo clippy -p klipolo-app --all-targets --locked -- -D warnings
# What only trial builds compile (the slow-disk hook `render::pool::slow_stats`, the decoders' measurement counters),
# linted too so it can't rot.
run "clippy engine (trial)" cargo clippy -p klipolo-engine --all-targets --features trial --locked -- -D warnings
# The recorder's real backends as Linux builds them (the stub, with tray-icon and global-hotkey), compiled so they
# can't rot before the app depends on them; nothing here runs them.
run "check capture (real-capture)" cargo check -p klipolo-capture --features real-capture --locked
# The Meta client's tests answer from a local mock server; nothing here reaches Meta. The recorder's tests use
# its synthetic sources and fake backend (the kill sweep included); nothing here captures.
run "test model, cursor, platform, fonts, speech-wire, meta, sam, analysis, chat, mcp, capture" cargo test \
  -p klipolo-model -p klipolo-cursor -p klipolo-platform -p klipolo-fonts -p klipolo-speech-wire -p klipolo-meta \
  -p klipolo-sam -p klipolo-analysis -p klipolo-chat -p klipolo-mcp -p klipolo-capture --features klipolo-capture/synthetic --locked \
  --no-fail-fast
run "test app" cargo test -p klipolo-app --locked --no-fail-fast
# The fonts crate's download tests against a local server (`test-hosts`); nothing here reaches Google.
run "test fonts (local download server)" cargo test -p klipolo-fonts --features test-hosts --locked
# Export tests that need an H.264 encoder skip themselves: the LGPL FFmpeg has none on Linux.
run "test engine (no GPU)" cargo test -p klipolo-engine --locked --no-fail-fast
# The fixture tests the default run ignores, on the CPU: FFmpeg software decode, in-process probe, the PCM cache.
run "test engine fixtures (no GPU)" cargo test -p klipolo-engine --locked --no-fail-fast \
  --test decode --test probe --test audio --test video_start -- --ignored --exact \
  decodes_a_frame_from_the_fixture unknown_fps_is_probed imports_probe_without_an_ffprobe_tool pcm_cache_keeps_mono_mono \
  ffmpeg_and_the_stored_index_start_the_picture_at_the_same_place
# The 16 kHz WAV for speech from the fixtures (a clip with sound, a stereo one, one with none).
run "test analysis fixtures" cargo test -p klipolo-analysis --locked --test audio --test cli -- --ignored
# Import on the fixtures: unreadable files named with why, a portrait phone video's rotation and an NTSC rate kept.
run "test app fixtures" cargo test -p klipolo-app --locked -- --ignored
# The generated bindings and code lists are committed (the release build runs `pnpm build` from a clean checkout):
# written again, they must match the checkout, files added or gone included.
run "bindings are current" scripts/check-bindings.sh "$generated"
# The page: its packages (pnpm's store kept in .build, as the Rust builds are in target/), typecheck and tests (jsdom).
run "pnpm install" pnpm install --frozen-lockfile --store-dir .build/pnpm-store
# The third-party notices the app ships list what Cargo.lock and the page's build hold: after a dependency change, run
# scripts/release/third-party.sh and commit its file (a few seconds: crates are fetched, not built).
run "third-party notices are current" scripts/release/third-party.sh --check
# Cap-MIT.txt's first line names exactly the files from Cap's MIT crates (scripts/release/lib.sh's cap_mit_check): a
# file vendored, renamed or removed without it fails here, not at a release build's bundle check.
run "Cap's MIT licence names its files" \
  bash -c '. scripts/release/lib.sh && cap_mit_check . "$1" "$1"' _ crates/klipolo-app/licenses/Cap-MIT.txt
run "page typecheck" pnpm -C apps/desktop typecheck
run "page tests" pnpm -C apps/desktop test
# What the engine's probe reads for sync (the sound lead, creation time and audio format), on the fixtures.
run "test engine sync inputs (fixtures)" cargo test -p klipolo-engine --locked --test sync_inputs -- --ignored
# KLIPOLO_REQUIRE_GPU: a GPU test that finds no adapter fails here instead of skipping (`test_support::adapter_or_skip`).
run "GPU smoke on lavapipe" env WGPU_BACKEND=vulkan KLIPOLO_REQUIRE_GPU=1 \
  cargo test -p klipolo-engine --locked --test gpu -- --ignored --exact black_layer_fades_what_is_below
# Every other GPU test on lavapipe: the compositor, video, image and text layers, effects and transitions, the cursor
# pass, export's cursors against the preview's, each caption style's golden, agent WGSL's hardening, filter padding,
# ABI v1.1's generators, the render worker, an export job's contributions, the guards, scene layers, paths, strokes,
# stencils and text, the shader presets' goldens, and the title templates in the preview and an export. (text_atlas
# reads Metal's own objects and stays macOS-only; the cursor's GPU time, export overhead and scene frame costs are
# measurements, run by hand.)
run "GPU tests on lavapipe" env WGPU_BACKEND=vulkan KLIPOLO_REQUIRE_GPU=1 \
  cargo test -p klipolo-engine --locked --no-fail-fast --test gpu --test effects --test cursor_gpu --test export_cursor \
  --test caption_goldens \
  --test harden_gpu --test padding_gpu --test abi_v11_gpu --test make_worker_gpu --test make_measure_gpu --test make_scene_gpu \
  --test export_contrib_gpu --test guards_gpu --test scene_gpu --test scene_clips_gpu \
  --test scene_paths_gpu --test scene_strokes_gpu --test scene_stencils_gpu --test scene_text_gpu \
  --test scene_text_motion_gpu --test scene_text_cache_gpu --test scene_verified_gpu --test scene_text_gone_gpu \
  --test scene_layer_scale_gpu --test scene_groups_gpu --test scene_fx_gpu --test scene_groups_caps_gpu \
  --test scene_memory_gpu --test scene_order_gpu --test scene_bounds_gpu --test scene_root_gpu --test scene_geometry_gpu --test scene_part_gpu \
  --test presets_gpu --test scene_presets \
  -- --ignored --skip black_layer_fades_what_is_below --skip cursor_pass_gpu_time --skip export_overhead_with_the \
  --skip preparing_a_2_hour_take --skip scene_frame_costs
# A caption frame drawn through an export job and its worker's start-up matches the preview's, per style.
run "caption export vs preview on lavapipe" env WGPU_BACKEND=vulkan KLIPOLO_REQUIRE_GPU=1 \
  cargo test -p klipolo-engine --locked --test caption_goldens -- --exact export_and_preview_draw_the_same_caption_frame
# The export worker waits for the system font scan before frame 0, and so does an export's scene text on its build
# thread, while the render worker's scene text never waits for it: each its own binary, as the scan is once a process.
run "export fonts on lavapipe" env WGPU_BACKEND=vulkan KLIPOLO_REQUIRE_GPU=1 cargo test -p klipolo-engine --locked \
  --test export_fonts --test scene_text_fonts --test worker_scene_text
# The slow-disk tests, which only trial builds compile (`render::pool::slow_stats` makes a file's stat slow), and the
# stalled software decoder's, fixtures included, and the GPU one on lavapipe (the preview's redraw signal stays wired to
# its decoder pool). They time stats of 1-3 s, so two test threads at most: a busy machine mustn't make them flaky.
run "test slow disks (trial; GPU on lavapipe)" env WGPU_BACKEND=vulkan KLIPOLO_REQUIRE_GPU=1 \
  cargo test -p klipolo-engine --locked --no-fail-fast --features trial --test decode --test gpu --test stalls -- \
  --include-ignored --test-threads=2 a_disk_ a_slow_ drawn_again_when_it_answers

echo; echo "=== Summary"
printf '%s\n' "${results[@]}"
! printf '%s\n' "${results[@]}" | grep -q '^FAIL'
