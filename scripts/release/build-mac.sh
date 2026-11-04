#!/usr/bin/env bash
# A release build of Klipolo for one Mac architecture (both build on an Intel Mac; aarch64 is cross-compiled):
#   scripts/release/build-mac.sh <x86_64|aarch64> <version>
# writes dist/Klipolo-<version>-<arch>.dmg: Developer ID signed with the hardened runtime, notarized and stapled.
# KLIPOLO_SOURCE_URL must be the public https:// address of Klipolo's source code: NOTICE.txt gives it with the commit
# built (the AGPL's source offer for this version), and check-bundle.sh fails a bundle whose NOTICE.txt still has the
# {{SOURCE_URL}} or {{SOURCE_COMMIT}} placeholder. That commit must be pushed there, tagged v<version>, before the disk
# image is shared.
# KLIPOLO_TRIAL_CODE is the relay access code built into the app as its free credit (read at compile time: klipolo-app's
# credentials::built_in_trial_code), so AI works with no key or code to type. A release without it is refused unless
# KLIPOLO_NO_TRIAL=1 asks for a copy with no free credit. The script says which ("trial credit: built in" or "none"),
# checks the built app holds the code, and never prints the code.
# 1. Checks: the source code address, a clean checkout (a release builds a commit), 8 GB free, rustup's cargo with the
#    target installed, and the LGPL FFmpeg prefix for the arch in .build/ffmpeg-9.0 (scripts/link-ffmpeg.sh), checked
#    by scripts/verify-ffmpeg-prefix.sh: exactly the prefix that ships.
# 2. The licences into .build/release/<arch>/licenses: every file in crates/klipolo-app/licenses (Klipolo's notice,
#    with the source code address; the third-party notices, scripts/release/third-party.sh; Cap's MIT licence for the
#    recorder's files from Cap's MIT crates, Cap-MIT.txt; the LGPL text), Klipolo's AGPL text, FFmpeg's LGPL notice
#    with the configure line of this arch's prefix (arm64 adds the cross-compile flags), Archivo's OFL.
# 3. scripts/release/third-party.sh --check after `pnpm install`: the notices that ship list this checkout's crates and
#    the page's packages. Then `pnpm tauri build --target <triple> --bundles app` (its beforeBuildCommand runs
#    `pnpm build`), signed ad hoc with the hardened runtime and the app's entitlements (tauri.conf.json's
#    bundle.macOS.entitlements). FFMPEG_DIR, PKG_CONFIG_LIBDIR and KLIPOLO_FFMPEG_PREFIX point at this arch's prefix,
#    and --config lists its dylibs (by install name) as the bundle's frameworks and adds the licences. Source paths
#    are remapped, so the binary names no home folder. Apple's notary variables are unset: notarization is
#    notarize.sh's.
# 4. The bundle is checked before it is signed with the Developer ID: its version and minimum macOS here, then
#    check-bundle.sh (the licences and fonts, the permission strings, the ad hoc signature's camera and microphone
#    entitlements, every Mach-O built for this arch only, load paths and rpaths).
# 5. sign.sh, notarize.sh (the app, then the disk image), then check-dmg.sh mounts the disk image read-only and
#    checks it: spctl must say `accepted` with `source=Notarized Developer ID`.
# CARGO_BUILD_JOBS defaults to 4 here (the Mac is also someone's daily machine); run it under `nice -n 10`.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
repo=$(cd "$here/../.." && pwd -P)
. "$here/lib.sh"
usage() { echo "usage: $0 <x86_64|aarch64> <version>" >&2; exit 2; }
[ $# -eq 2 ] || usage
arch=$1 version=$2
case "$arch" in
  x86_64) triple=x86_64-apple-darwin ffarch=x86_64 ;;
  aarch64) triple=aarch64-apple-darwin ffarch=arm64 ;;
  *) usage ;;
esac
is_version "$version" || { echo "refused: not a version: $version" >&2; exit 2; }
is_source_url "${KLIPOLO_SOURCE_URL:-}" || {
  echo "refused: set KLIPOLO_SOURCE_URL to the public https:// address of Klipolo's source code (NOTICE.txt gives it)" >&2
  exit 2
}
# The free credit's code without the spaces around it, as the app reads it (built_in_trial_code trims it too).
trial_code=${KLIPOLO_TRIAL_CODE:-}
trial_code=${trial_code#"${trial_code%%[![:space:]]*}"}
trial_code=${trial_code%"${trial_code##*[![:space:]]}"}
if [ -z "$trial_code" ] && [ "${KLIPOLO_NO_TRIAL:-}" != 1 ]; then
  echo "refused: set KLIPOLO_TRIAL_CODE (or KLIPOLO_NO_TRIAL=1 for a build without free credit)" >&2
  exit 2
fi
cd "$repo"
started=$(date +%s)

# 1. Checks.
[ -z "$(git status --porcelain)" ] || {
  echo "refused: the checkout has changes or untracked files; a release builds a commit" >&2
  git status --porcelain >&2
  exit 2
}
free_gb=$(($(df -Pk "$repo" | awk 'NR == 2 {print $4}') / 1024 / 1024))
[ "$free_gb" -ge 8 ] || { echo "refused: $free_gb GB free; a release build needs 8 GB" >&2; exit 2; }
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
[ "$(command -v cargo)" = "${CARGO_HOME:-$HOME/.cargo}/bin/cargo" ] || { echo "refused: cargo isn't rustup's" >&2; exit 2; }
rustup target list --installed | grep -qx "$triple" || { echo "refused: run rustup target add $triple" >&2; exit 2; }
ff=$repo/.build/ffmpeg-9.0
prefix=$ff/$ffarch/prefix
[ -d "$prefix/lib" ] || { echo "refused: no FFmpeg prefix at $prefix (scripts/link-ffmpeg.sh)" >&2; exit 2; }
"$repo/scripts/verify-ffmpeg-prefix.sh" "$ff/$ffarch/logs" "$prefix" "$ffarch"

# 2. The licences.
stage=$repo/.build/release/$arch
rm -rf "$stage"
mkdir -p "$stage/licenses"
cp crates/klipolo-app/licenses/* "$stage/licenses/"
# The clean checkout (step 1) is the source: NOTICE.txt names its commit.
fill_source "$stage/licenses/NOTICE.txt" "$KLIPOLO_SOURCE_URL" "$(git rev-parse HEAD)"
cp LICENSE "$stage/licenses/LICENSE.txt"
cp apps/desktop/src/styles/fonts/OFL.txt "$stage/licenses/Archivo-OFL.txt"
ffversion=$(sed -n 's/^VERSION=//p' scripts/ffmpeg-common.sh)
ffsha=$(sed -n 's/^SHA256=//p' scripts/ffmpeg-common.sh)
configure=$(sed -n 's/^#define FFMPEG_CONFIGURATION "\(.*\)"$/\1/p' "$ff/$ffarch/logs/config.h")
[ -n "$ffversion" ] && [ -n "$ffsha" ] && [ -n "$configure" ] || { echo "refused: FFmpeg's version or configure line is missing" >&2; exit 1; }
cat > "$stage/licenses/FFmpeg-LGPL.txt" << EOF
Klipolo uses FFmpeg $ffversion (https://ffmpeg.org), licensed under the GNU Lesser General Public License,
version 2.1 or later. The licence text is in COPYING.LGPLv2.1 next to this file.

FFmpeg is dynamically linked. Its libraries are the separate files in Klipolo.app/Contents/Frameworks
(libavcodec, libavformat, libavutil, libavfilter, libswscale, libswresample). You may replace them with your own
modified build of these libraries, and Klipolo will use it. macOS checks the app's signature, so after replacing
them, re-sign the app on your Mac: codesign --force --deep --sign - /Applications/Klipolo.app
The re-signed app is no longer the notarized one, so macOS 15 won't open a downloaded copy until you remove the
download mark: xattr -dr com.apple.quarantine /Applications/Klipolo.app
Or open it once, then allow it in System Settings, Privacy & Security.

The exact FFmpeg source used for this build is ffmpeg-$ffversion.tar.xz from https://ffmpeg.org/releases/
(SHA-256 $ffsha). Klipolo's FFmpeg build script, scripts/build-ffmpeg.sh, is part of Klipolo's source code.

This copy of Klipolo is for $arch Macs ($ffarch). FFmpeg was configured with:
$configure
EOF

# 3. The app, signed ad hoc with the hardened runtime and the app's entitlements.
frameworks=""
for f in "$prefix"/lib/lib*.dylib; do
  [ -L "$f" ] && continue
  name=$(otool -D "$f" | tail -1)
  frameworks+="${frameworks:+,}\"../../.build/ffmpeg-9.0/$ffarch/prefix/lib/${name#@rpath/}\""
done
config="{\"version\":\"$version\",\"bundle\":{\"macOS\":{\"signingIdentity\":\"-\",\"hardenedRuntime\":true,"
config+="\"frameworks\":[$frameworks]},\"resources\":{\"../../.build/release/$arch/licenses/\":\"licenses/\"}}}"
export FFMPEG_DIR=$prefix KLIPOLO_FFMPEG_PREFIX=$prefix PKG_CONFIG_LIBDIR=$prefix/lib/pkgconfig PKG_CONFIG_PATH=
export PKG_CONFIG_ALLOW_CROSS=1 MACOSX_DEPLOYMENT_TARGET=15.0 CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
# Passed through to cargo (rustc reads it for option_env!, and rebuilds the app when it changes); never printed.
if [ -n "$trial_code" ]; then
  export KLIPOLO_TRIAL_CODE=$trial_code
  echo "trial credit: built in"
else
  export KLIPOLO_TRIAL_CODE=
  echo "trial credit: none"
fi
home=$(cd ~ && pwd -P)
cargo_home=$(cd "${CARGO_HOME:-$home/.cargo}" && pwd -P)
# rustc applies the last matching prefix: the repository (under the home folder) maps to ".", the rest of it to "~".
CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$home=~"$'\x1f'"--remap-path-prefix=$cargo_home=~/.cargo"
CARGO_ENCODED_RUSTFLAGS+=$'\x1f'"--remap-path-prefix=$repo=."
export CARGO_ENCODED_RUSTFLAGS
export CFLAGS="${CFLAGS:+$CFLAGS }-ffile-prefix-map=$home=~ -ffile-prefix-map=$repo=."
pnpm install --frozen-lockfile
"$here/third-party.sh" --check
built=$(date +%s)
(cd apps/desktop && env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID -u APPLE_API_KEY -u APPLE_API_ISSUER \
  -u APPLE_API_KEY_PATH -u APPLE_CERTIFICATE -u APPLE_CERTIFICATE_PASSWORD -u APPLE_SIGNING_IDENTITY \
  pnpm tauri build --ci --target "$triple" --bundles app --config "$config")
echo "built in $(($(date +%s) - built)) s"
app=$repo/target/$triple/release/bundle/macos/Klipolo.app
[ -d "$app" ] || { echo "refused: no app at $app" >&2; exit 1; }

# 4. The bundle, before it is signed with the Developer ID.
plist=$app/Contents/Info.plist
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")" = "$version" ] || { echo "FAIL: the app isn't $version" >&2; exit 1; }
[ "$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$plist")" = 15.0 ] || { echo "FAIL: LSMinimumSystemVersion isn't 15.0" >&2; exit 1; }
"$here/check-bundle.sh" "$app" "$ffarch" || { echo "the bundle check failed: nothing was signed with the Developer ID" >&2; exit 1; }
# The free credit is in the app: the code is read from a pipe (never an argument, so never in the process list) and
# only the count of matches is printed.
if [ -n "$trial_code" ]; then
  main=$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$plist")
  found=$(grep -a -c -F -f <(printf '%s\n' "$trial_code") "$main" || true)
  [ "${found:-0}" -gt 0 ] || { echo "FAIL: the app binary doesn't hold the free credit's code" >&2; exit 1; }
  echo "trial credit: in the app binary ($found matches)"
fi

# 5. Signed, notarized, stapled, then checked as a user's Mac would.
"$here/sign.sh" "$app"
out=$repo/dist/Klipolo-$version-$arch.dmg
"$here/notarize.sh" "$app" "$out"
"$here/check-dmg.sh" "$out"
echo "commit $(git rev-parse --short HEAD), $(($(date +%s) - started)) s in all"
