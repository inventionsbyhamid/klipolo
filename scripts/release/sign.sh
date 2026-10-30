#!/usr/bin/env bash
# Developer ID signing from the inside out, after a build signed ad hoc:
#   scripts/release/sign.sh <Klipolo.app>
# Every Mach-O under Contents/ other than the main executable (the FFmpeg dylibs in Frameworks/, any helper), and any
# nested bundle after what it holds, deepest first, each with `codesign --force --options runtime --timestamp`; then
# the app itself with crates/klipolo-app/entitlements.plist; then `codesign --verify --deep --strict`. The identity
# comes from lib.sh's developer_id and is never printed.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
repo=$(cd "$here/../.." && pwd -P)
. "$here/lib.sh"
[ $# -eq 1 ] || { echo "usage: $0 <Klipolo.app>" >&2; exit 2; }
app=${1%/}
[ -f "$app/Contents/Info.plist" ] || { echo "refused: not an app bundle: $app" >&2; exit 2; }
main=$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")
[ -f "$main" ] || { echo "refused: $app has no main executable at $main" >&2; exit 2; }
entitlements=$repo/crates/klipolo-app/entitlements.plist
plutil -lint -s "$entitlements"
# The parsed plist, so a comment that names the key doesn't count.
if plutil -convert xml1 -o - "$entitlements" | grep -q 'get-task-allow'; then
  echo "refused: $entitlements allows get-task-allow" >&2
  exit 2
fi

# Everything to sign before the bundle, deepest first (by the number of slashes), then by path for a stable order.
order() {
  local f
  while IFS= read -r -d '' f; do
    [ "$f" = "$main" ] && continue
    if [ -d "$f" ]; then
      case "$f" in *.app | *.framework | *.xpc | *.appex | *.bundle) ;; *) continue ;; esac
    elif [ -L "$f" ] || ! is_macho "$f"; then
      continue
    fi
    printf '%s\t%s\n' "$(printf '%s' "$f" | tr -cd / | wc -c | tr -d ' ')" "$f"
  done < <(find "$app/Contents" -mindepth 1 \( -type f -o -type d \) -print0) \
    | LC_ALL=C sort -t "$(printf '\t')" -k1,1nr -k2,2 | cut -f2-
}

id=$(developer_id)
sign() {
  codesign --force --options runtime --timestamp --sign "$id" "$@" 2>&1 | redact "$id"
}
count=0
while IFS= read -r f; do
  sign "$f"
  count=$((count + 1))
done < <(order)
sign --entitlements "$entitlements" "$app"
codesign --verify --deep --strict --verbose=2 "$app" 2>&1 | redact "$id"
echo "signed $count nested files, then the app"
