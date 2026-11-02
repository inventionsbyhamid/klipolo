#!/usr/bin/env bash
# Klipolo's third-party notices: every Rust crate the app's binary links on either Mac architecture, and every
# JavaScript package in the page's build, grouped by licence, with each licence's full text once (SPDX's texts, kept in
# scripts/release/licenses), and the bundled pack's sounds and pictures made by others (assets/library/manifest.json).
# build-mac.sh ships the file as Klipolo.app/Contents/Resources/licenses/THIRD-PARTY-NOTICES.txt, next to NOTICE.txt,
# and check-bundle.sh requires it there.
#   scripts/release/third-party.sh           writes crates/klipolo-app/licenses/THIRD-PARTY-NOTICES.txt again
#   scripts/release/third-party.sh --check   fails when that file isn't what it would write (the Linux gate runs this)
# Needs cargo (crates are fetched, not built) and the page's packages (pnpm install). It runs the page's Vite build in
# memory and writes nothing else. Linux and macOS give the same file; on a Mac, run it under the build lock.
# Run it after any change to Cargo.lock, pnpm-lock.yaml or the pack's manifest, and commit the file. A package or pack
# file whose licences have no text in scripts/release/licenses stops it: check that the licence can ship in an
# AGPL-3.0-only app, then add its SPDX text (the ones there are text/<id>.txt from
# https://github.com/spdx/license-list-data at tag v3.29.0).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
repo=$(cd "$here/../.." && pwd -P)
out=crates/klipolo-app/licenses/THIRD-PARTY-NOTICES.txt
check=0
case "${1:-}" in
  --check) check=1 ;;
  "") ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac
cd "$repo"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo metadata --locked --format-version 1 \
  --filter-platform x86_64-apple-darwin --filter-platform aarch64-apple-darwin > "$tmp/metadata.json"
node "$here/third-party.mjs" "$tmp/metadata.json" apps/desktop "$here/licenses" assets/library/manifest.json \
  > "$tmp/notices.txt"
summary=$(sed -n 's/^\([0-9]* Rust crates and [0-9]* JavaScript packages\).*/\1/p' "$tmp/notices.txt")
if [ "$check" = 1 ]; then
  if cmp -s "$tmp/notices.txt" "$out"; then
    echo "third-party notices are current: $summary"
    exit 0
  fi
  diff -u "$out" "$tmp/notices.txt" | head -60 || true
  echo "FAIL: $out isn't current: run scripts/release/third-party.sh and commit it" >&2
  exit 1
fi
cp "$tmp/notices.txt" "$out"
echo "wrote $out: $summary, $(wc -c < "$out" | tr -d ' ') bytes"
