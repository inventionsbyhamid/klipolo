#!/usr/bin/env bash
# Notarization, after sign.sh:
#   scripts/release/notarize.sh <signed Klipolo.app> <out.dmg>
# 1. The app: zipped with ditto, `notarytool submit --wait`, then stapled.
# 2. The disk image, made from the stapled app: the app and a link to /Applications, volume name Klipolo.
# 3. The disk image: signed with the Developer ID, submitted the same way, stapled, then moved to <out.dmg> once whole.
# Anything but Accepted prints Apple's log for that submission (it names the file at fault) and fails.
# Credentials: the notarytool keychain profile KLIPOLO_NOTARY_PROFILE (default `klipolo`, made once with
# `xcrun notarytool store-credentials`). Neither it nor the identity is ever printed.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
. "$here/lib.sh"
[ $# -eq 2 ] || { echo "usage: $0 <signed Klipolo.app> <out.dmg>" >&2; exit 2; }
app=${1%/} dmg=$2
[ -f "$app/Contents/Info.plist" ] || { echo "refused: not an app bundle: $app" >&2; exit 2; }
case "$dmg" in *.dmg) ;; *) echo "refused: not a .dmg name: $dmg" >&2; exit 2 ;; esac
name=$(basename "$app")
profile=${KLIPOLO_NOTARY_PROFILE:-klipolo}
id=$(developer_id)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Submits a file and waits. Prints the submission id, status and time taken; anything but Accepted fails.
submit() {
  local file=$1 out=$work/submit.json started status submission
  started=$(date +%s)
  xcrun notarytool submit "$file" --keychain-profile "$profile" --wait --timeout 2h --output-format json > "$out" || true
  status=$(plutil -extract status raw -o - "$out" 2> /dev/null || true)
  submission=$(plutil -extract id raw -o - "$out" 2> /dev/null || true)
  echo "notary: $(basename "$file") submission ${submission:-none} status ${status:-no answer} after $(($(date +%s) - started)) s"
  if [ "$status" != Accepted ]; then
    cat "$out" >&2
    if [ -n "$submission" ]; then
      xcrun notarytool log "$submission" --keychain-profile "$profile" "$work/log.json" > /dev/null && cat "$work/log.json" >&2
    fi
    exit 1
  fi
}

# 1. The app.
ditto -c -k --keepParent "$app" "$work/app.zip"
submit "$work/app.zip"
xcrun stapler staple "$app"
xcrun stapler validate "$app"

# 2. The disk image from the stapled app.
mkdir "$work/image"
ditto "$app" "$work/image/$name"
ln -s /Applications "$work/image/Applications"
made=$work/$(basename "$dmg")
hdiutil create -volname Klipolo -srcfolder "$work/image" -fs HFS+ -format UDZO -ov "$made" > /dev/null

# 3. The disk image signed, notarized and stapled, then put in place whole.
codesign --force --timestamp --sign "$id" "$made" 2>&1 | redact "$id"
submit "$made"
xcrun stapler staple "$made"
xcrun stapler validate "$made"
mkdir -p "$(dirname "$dmg")"
mv -f "$made" "$dmg"
echo "$dmg"
