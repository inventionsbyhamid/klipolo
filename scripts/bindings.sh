#!/usr/bin/env bash
# `pnpm bindings` (in apps/desktop): writes the page's generated files again from the crates' `export_bindings` tests.
# They are committed, so `pnpm build` works from a clean checkout (the release build); the Linux gate runs this and fails
# when the result differs from the commit.
#   apps/desktop/src/bindings: the TypeScript types (ts-rs, TS_RS_EXPORT_DIR in .cargo/config.toml) and the code lists
#     (modelCodes.json, fontsCodes.json) the page words Rust's codes from. Emptied first, so a type that went goes too.
#   apps/desktop/src/lib: caption-styles.json and font-fallbacks.json. The parity files there are checked by their own
#     tests and rewritten only with KLIPOLO_UPDATE_PARITY=1 or UPDATE_GOLDEN=1.
# The engine and the app link FFmpeg: on a Mac, scripts/link-ffmpeg.sh first; the app's types compile the whole app, so
# run it on the Linux build server (scripts/ci-linux.sh's environment) rather than on a busy Mac.
set -euo pipefail
cd "$(dirname "$0")/.."
rm -rf apps/desktop/src/bindings
cargo test -p klipolo-model --locked export_bindings
cargo test -p klipolo-platform --locked export_bindings
cargo test -p klipolo-fonts --locked export_bindings
cargo test -p klipolo-engine --locked --lib export_bindings
cargo test -p klipolo-app --locked export_bindings
echo "bindings: $(ls apps/desktop/src/bindings | wc -l | tr -d ' ') files in apps/desktop/src/bindings"
