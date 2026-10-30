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

# 2. The disk image from the stapled app, filled at a private mount point and then compressed. Never through
# `hdiutil create -srcfolder`, which mounts at /Volumes/Klipolo: once a notarized Klipolo has been opened from there,
# macOS refuses to write a new Klipolo.app at that path ("Operation not permitted").
mkdir "$work/mnt"
size_mb=$(($(du -sm "$app" | cut -f1) * 12 / 10 + 20))
hdiutil create -size "${size_mb}m" -fs HFS+ -volname Klipolo -ov "$work/image.dmg" > /dev/null
hdiutil attach -nobrowse -noautoopen -mountpoint "$work/mnt" "$work/image.dmg" > /dev/null
trap 'hdiutil detach -quiet -force "$work/mnt" 2> /dev/null || true; rm -rf "$work"' EXIT
ditto "$app" "$work/mnt/$name"
ln -s /Applications "$work/mnt/Applications"
hdiutil detach -quiet "$work/mnt"
made=$work/$(basename "$dmg")
hdiutil convert "$work/image.dmg" -format UDZO -o "$made" > /dev/null

# 3. The disk image signed, notarized and stapled, then put in place whole.
codesign --force --timestamp --sign "$id" "$made" 2>&1 | redact "$id"
submit "$made"
xcrun stapler staple "$made"
xcrun stapler validate "$made"
mkdir -p "$(dirname "$dmg")"
mv -f "$made" "$dmg"
echo "$dmg"
