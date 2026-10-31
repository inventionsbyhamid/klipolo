#!/usr/bin/env bash
# The app bundle's check before anything is signed (build-mac.sh runs it; it works on a signed app too):
#   scripts/release/check-bundle.sh <Klipolo.app> <x86_64|arm64>
# Fails unless the licences (the third-party notices too, none of them empty) and the fonts are in Resources, every
# Mach-O is built for <arch> only and for macOS 15.0 or older, every load path is @rpath/, @executable_path/,
# @loader_path/, /usr/lib/ or /System/Library/, and every LC_RPATH is relative (@executable_path/ or @loader_path/),
# never a machine path such as the dev FFmpeg prefix's.
# The app must also find its libraries: the main executable's LC_RPATHs include @executable_path/../Frameworks, and
# every @rpath/<name> that any Mach-O in the bundle loads is a file at Contents/Frameworks/<name>. And no file under
# Contents names the builder's home folder or user name, nor does the page as built in apps/desktop/dist: the main
# executable holds the page brotli-compressed, so searching the binary can't see into it. NOTICE.txt must give the
# source code's https:// address, its {{SOURCE_URL}} placeholder filled in.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
repo=$(cd "$here/../.." && pwd -P)
. "$here/lib.sh"
usage() { echo "usage: $0 <Klipolo.app> <x86_64|arm64>" >&2; exit 2; }
[ $# -eq 2 ] || usage
app=${1%/} arch=$2
case "$arch" in x86_64 | arm64) ;; aarch64) arch=arm64 ;; *) usage ;; esac
[ -f "$app/Contents/Info.plist" ] || { echo "refused: not an app bundle: $app" >&2; exit 2; }
main=$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")
[ -f "$main" ] || { echo "refused: $app has no main executable at $main" >&2; exit 2; }
home=$(cd ~ && pwd -P)
rc=0
fail() { echo "FAIL: $*"; rc=1; }
is_macho "$main" || fail "the main executable ${main#"$app"/} isn't a Mach-O file"

for f in FFmpeg-LGPL.txt COPYING.LGPLv2.1 LICENSE.txt NOTICE.txt THIRD-PARTY-NOTICES.txt; do
  if [ ! -f "$app/Contents/Resources/licenses/$f" ]; then
    fail "licenses/$f isn't in the app"
  elif [ ! -s "$app/Contents/Resources/licenses/$f" ]; then
    fail "licenses/$f is empty"
  fi
done
[ -d "$app/Contents/Resources/fonts" ] || fail "the fonts aren't in the app"
# NOTICE.txt gives the public address of the source code (the AGPL's source offer). build-mac.sh fills its
# {{SOURCE_URL}} placeholder from KLIPOLO_SOURCE_URL; a bundle that still has the placeholder can't ship.
licences=$app/Contents/Resources/licenses
if [ -f "$licences/NOTICE.txt" ]; then
  if grep -rqF '{{SOURCE_URL}}' "$licences"; then
    fail "licenses/NOTICE.txt still has the {{SOURCE_URL}} placeholder: build with KLIPOLO_SOURCE_URL set"
  elif source_url=$(sed -n 's/^Source code: //p' "$licences/NOTICE.txt") && is_source_url "$source_url"; then
    echo "-- source code: $source_url"
  else
    fail "licenses/NOTICE.txt gives no https:// address for the source code (one 'Source code: ' line)"
  fi
fi

files=$(mktemp)
trap 'rm -f "$files"' EXIT
find "$app/Contents" -type f -print0 > "$files"
count=0
while IFS= read -r -d '' f; do
  is_macho "$f" || continue
  count=$((count + 1))
  rel=${f#"$app"/}
  archs=$(lipo -archs "$f")
  minos=$(otool -l "$f" | awk '/LC_BUILD_VERSION/ {b = 1} b && $1 == "minos" {print $2; exit}')
  echo "-- $rel [$archs, macOS $minos]"
  [ "$archs" = "$arch" ] || fail "$rel is built for $archs, not $arch"
  awk -v v="$minos" 'BEGIN { split(v, p, "."); exit !(p[1] < 15 || (p[1] == 15 && p[2] + 0 == 0)) }' \
    || fail "$rel needs macOS $minos, newer than 15.0"
  bad=$(otool -L "$f" | tail -n +2 | awk '{print $1}' \
    | grep -vE '^(@rpath/|@executable_path/|@loader_path/|/usr/lib/|/System/Library/)' || true)
  [ -z "$bad" ] || fail "$rel loads from outside the app and the system: $bad"
  rpaths=$(otool -l "$f" | awk '$1 == "cmd" {r = ($2 == "LC_RPATH")} r && $1 == "path" {print $2}')
  for r in $rpaths; do
    echo "   LC_RPATH $r"
    case "$r" in @executable_path/* | @loader_path/*) ;; *) fail "$rel has a machine LC_RPATH: $r" ;; esac
  done
  own=$(otool -D "$f" | tail -n +2)
  for dep in $(otool -L "$f" | tail -n +2 | awk '{print $1}' | grep '^@rpath/' || true); do
    [ "$dep" = "$own" ] && continue
    [ -f "$app/Contents/Frameworks/${dep#@rpath/}" ] || fail "$rel loads $dep, but Contents/Frameworks/${dep#@rpath/} isn't there"
  done
done < "$files"
[ "$count" -gt 1 ] || fail "only $count Mach-O files in the app"
# The main executable's own, whether or not the walk above met it: one of its LC_RPATHs is exactly this text.
main_rpaths=$(otool -l "$main" | awk '$1 == "cmd" {r = ($2 == "LC_RPATH")} r && $1 == "path" {print $2}' || true)
grep -qxF '@executable_path/../Frameworks' <<< "$main_rpaths" \
  || fail "${main#"$app"/} has no @executable_path/../Frameworks LC_RPATH, so it can't find the libraries in Frameworks"

# No file under Contents (binaries, resources) or in the page's build output names the builder's home folder or user
# name (lib.sh's naming_files: the user name only as a whole path part). The page is the one `pnpm build` wrote for this
# bundle (build-mac.sh checks right after building); without it the check fails rather than skip the page.
page=$repo/apps/desktop/dist
searched=("$app/Contents")
if [ -f "$page/index.html" ]; then searched+=("$page"); else fail "no page build at $page to search (pnpm build)"; fi
set +e
named=$(naming_files "$home" "$(id -un)" "${searched[@]}")
status=$?
set -e
# grep says 2 when it couldn't read a file, also when it found others: both are failures, and both are told.
[ -z "$named" ] || fail "these files name the builder's home folder or user name: $(tr '\n' ' ' <<< "$named")"
[ "$status" -le 1 ] || fail "couldn't read every file under Contents and the page to look for the home folder"

[ $rc -eq 0 ] || { echo "RESULT: FAIL" >&2; exit 1; }
echo "bundle check: $count Mach-O files, $arch only, relative rpaths only, every @rpath library in Frameworks," \
  "no file in the app or the page names the home folder or the user name"
