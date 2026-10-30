#!/usr/bin/env bash
# A finished disk image, checked as a user's Mac would (build-mac.sh runs it last; it needs no signing identity):
#   scripts/release/check-dmg.sh <Klipolo-<version>-<arch>.dmg>
# Mounts it read-only at a private mount point; spctl must accept the app inside with `source=Notarized Developer ID`;
# the app's signature verifies (deep, strict) and its ticket is stapled; then the disk image's own ticket. Prints the
# disk image's size and SHA-256.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
. "$here/lib.sh"
[ $# -eq 1 ] || { echo "usage: $0 <Klipolo.dmg>" >&2; exit 2; }
dmg=$1
[ -f "$dmg" ] || { echo "refused: no disk image at $dmg" >&2; exit 2; }
# The mount point goes away on every exit, also when the attach itself fails; a stuck volume is detached by force.
tmp=${TMPDIR:-/tmp}
mnt=$(mktemp -d "${tmp%/}/klipolo-dmg.XXXXXX")
trap 'hdiutil detach -quiet -force "$mnt" 2> /dev/null || true; rmdir "$mnt" 2> /dev/null || true' EXIT
hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$dmg" > /dev/null
# spctl exits non-zero on a rejection: its answer is still printed, and the check below fails.
assessment=$(spctl -a -vv -t exec "$mnt/Klipolo.app" 2>&1 | redact) || true
echo "$assessment"
grep -q ': accepted' <<< "$assessment" && grep -q 'source=Notarized Developer ID' <<< "$assessment" \
  || { echo "FAIL: Gatekeeper doesn't accept the app in the disk image" >&2; exit 1; }
codesign --verify --deep --strict --verbose=2 "$mnt/Klipolo.app" 2>&1 | redact
xcrun stapler validate "$mnt/Klipolo.app"
hdiutil detach -quiet "$mnt"
xcrun stapler validate "$dmg"
echo "$dmg"
echo "size $(stat -f %z "$dmg") bytes, sha256 $(shasum -a 256 "$dmg" | cut -d' ' -f1)"
