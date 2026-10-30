#!/usr/bin/env bash
# A release build of Klipolo for one Mac architecture (both build on an Intel Mac; aarch64 is cross-compiled):
#   scripts/release/build-mac.sh <x86_64|aarch64> <version>
# writes dist/Klipolo-<version>-<arch>.dmg: Developer ID signed with the hardened runtime, notarized and stapled.
# 1. Checks: a clean checkout (a release builds a commit), 8 GB free, rustup's cargo with the target installed, and the
#    LGPL FFmpeg prefix for the arch in .build/ffmpeg-9.0 (scripts/link-ffmpeg.sh), checked by
#    scripts/verify-ffmpeg-prefix.sh: exactly the prefix that ships.
# 2. The licences into .build/release/<arch>/licenses: Klipolo's notice and AGPL text, FFmpeg's LGPL notice with the
#    configure line of this arch's prefix (arm64 adds the cross-compile flags), the LGPL text, Archivo's OFL.
# 3. `pnpm tauri build --target <triple> --bundles app` (its beforeBuildCommand runs `pnpm build`), signed ad hoc with
#    the hardened runtime. FFMPEG_DIR, PKG_CONFIG_LIBDIR and KLIPOLO_FFMPEG_PREFIX point at this arch's prefix, and
#    --config lists its dylibs (by install name) as the bundle's frameworks and adds the licences. Source paths are
#    remapped, so the binary names no home folder. Apple's notary variables are unset: notarization is notarize.sh's.
# 4. The bundle is checked before anything is signed: every Mach-O built for this arch only, minimum macOS 15.0, load
#    paths only @rpath/, @executable_path/, @loader_path/, /usr/lib/ and /System/, and every LC_RPATH relative
#    (@executable_path/../Frameworks), never a machine path such as the dev prefix's.
# 5. sign.sh, notarize.sh (the app, then the disk image), then the disk image is mounted read-only and checked:
#    spctl must say `accepted` with `source=Notarized Developer ID`.
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

The exact FFmpeg source used for this build is ffmpeg-$ffversion.tar.xz from https://ffmpeg.org/releases/
(SHA-256 $ffsha). Klipolo's FFmpeg build script, scripts/build-ffmpeg.sh, is part of Klipolo's source code.

This copy of Klipolo is for $arch Macs ($ffarch). FFmpeg was configured with:
$configure
EOF

# 3. The app, signed ad hoc with the hardened runtime.
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
home=$(cd ~ && pwd -P)
cargo_home=$(cd "${CARGO_HOME:-$home/.cargo}" && pwd -P)
# rustc applies the last matching prefix: the repository (under the home folder) maps to ".", the rest of it to "~".
CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$home=~"$'\x1f'"--remap-path-prefix=$cargo_home=~/.cargo"
CARGO_ENCODED_RUSTFLAGS+=$'\x1f'"--remap-path-prefix=$repo=."
export CARGO_ENCODED_RUSTFLAGS
export CFLAGS="${CFLAGS:+$CFLAGS }-ffile-prefix-map=$home=~ -ffile-prefix-map=$repo=."
pnpm install --frozen-lockfile
built=$(date +%s)
(cd apps/desktop && env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID -u APPLE_API_KEY -u APPLE_API_ISSUER \
  -u APPLE_API_KEY_PATH -u APPLE_CERTIFICATE -u APPLE_CERTIFICATE_PASSWORD -u APPLE_SIGNING_IDENTITY \
  pnpm tauri build --ci --target "$triple" --bundles app --config "$config")
echo "built in $(($(date +%s) - built)) s"
app=$repo/target/$triple/release/bundle/macos/Klipolo.app
[ -d "$app" ] || { echo "refused: no app at $app" >&2; exit 1; }

# 4. The bundle, before anything is signed.
plist=$app/Contents/Info.plist
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")" = "$version" ] || { echo "FAIL: the app isn't $version" >&2; exit 1; }
[ "$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$plist")" = 15.0 ] || { echo "FAIL: LSMinimumSystemVersion isn't 15.0" >&2; exit 1; }
for f in FFmpeg-LGPL.txt COPYING.LGPLv2.1 LICENSE.txt NOTICE.txt; do
  [ -f "$app/Contents/Resources/licenses/$f" ] || { echo "FAIL: licenses/$f isn't in the app" >&2; exit 1; }
done
[ -d "$app/Contents/Resources/fonts" ] || { echo "FAIL: the fonts aren't in the app" >&2; exit 1; }
rc=0 count=0
while IFS= read -r -d '' f; do
  is_macho "$f" || continue
  count=$((count + 1))
  rel=${f#"$app"/}
  archs=$(lipo -archs "$f")
  minos=$(otool -l "$f" | awk '/LC_BUILD_VERSION/ {b = 1} b && $1 == "minos" {print $2; exit}')
  echo "-- $rel [$archs, macOS $minos]"
  [ "$archs" = "$ffarch" ] || { echo "FAIL: $rel is built for $archs, not $ffarch"; rc=1; }
  awk -v v="$minos" 'BEGIN { split(v, p, "."); exit !(p[1] < 15 || (p[1] == 15 && p[2] + 0 == 0)) }' \
    || { echo "FAIL: $rel needs macOS $minos, newer than 15.0"; rc=1; }
  bad=$(otool -L "$f" | tail -n +2 | awk '{print $1}' \
    | grep -vE '^(@rpath/|@executable_path/|@loader_path/|/usr/lib/|/System/Library/)' || true)
  [ -z "$bad" ] || { echo "FAIL: $rel loads from outside the app and the system: $bad"; rc=1; }
  rpaths=$(otool -l "$f" | awk '$1 == "cmd" {r = ($2 == "LC_RPATH")} r && $1 == "path" {print $2}')
  for r in $rpaths; do
    echo "   LC_RPATH $r"
    case "$r" in @executable_path/* | @loader_path/*) ;; *) echo "FAIL: $rel has a machine LC_RPATH: $r"; rc=1 ;; esac
  done
done < <(find "$app/Contents" -type f -print0)
[ "$count" -gt 1 ] || { echo "FAIL: only $count Mach-O files in the app" >&2; rc=1; }
[ $rc -eq 0 ] || { echo "the bundle check failed: nothing was signed" >&2; exit 1; }
home_paths=$(strings -a "$app/Contents/MacOS/"* | LC_ALL=C grep -c "$home/" || true)
echo "bundle check: $count Mach-O files, $ffarch only, relative rpaths only ($home_paths strings still name the home folder)"

# 5. Signed, notarized, stapled, then checked as a user's Mac would.
"$here/sign.sh" "$app"
out=$repo/dist/Klipolo-$version-$arch.dmg
"$here/notarize.sh" "$app" "$out"
id=$(developer_id)
mnt=$(mktemp -d)
hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$out" > /dev/null
trap 'hdiutil detach -quiet "$mnt" 2> /dev/null || true; rmdir "$mnt" 2> /dev/null || true' EXIT
assessment=$(spctl -a -vv -t exec "$mnt/Klipolo.app" 2>&1 | redact "$id")
echo "$assessment"
grep -q ': accepted' <<< "$assessment" && grep -q 'source=Notarized Developer ID' <<< "$assessment" \
  || { echo "FAIL: Gatekeeper doesn't accept the app in the disk image" >&2; exit 1; }
codesign --verify --deep --strict --verbose=2 "$mnt/Klipolo.app" 2>&1 | redact "$id"
xcrun stapler validate "$mnt/Klipolo.app"
hdiutil detach -quiet "$mnt"
xcrun stapler validate "$out"
echo "$out"
echo "size $(stat -f %z "$out") bytes, sha256 $(shasum -a 256 "$out" | cut -d' ' -f1)"
echo "commit $(git rev-parse --short HEAD), $(($(date +%s) - started)) s in all"
