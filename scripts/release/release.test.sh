#!/usr/bin/env bash
# The release checks' own tests, on a small app bundle made here (macOS with the Command Line Tools' clang; nothing is
# signed, nothing reaches the network, and it takes seconds):
#   scripts/release/release.test.sh
# The bundle is an executable that loads one dylib through @rpath, with the licences, a fonts folder, the bundled pack's
# manifest and licence, the app's own Info.plist keys and, through a stand-in for codesign, the app's own entitlements:
# check-bundle.sh must pass it. Then each broken copy must fail with its own message. check-bundle.sh runs from a copy
# of this folder in a checkout-shaped tree, so the test owns the page's build output and the source files it also
# searches.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
app_crate=$(cd "$here/../../crates/klipolo-app" && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/klipolo-release-test.XXXXXX")
work=$(cd "$work" && pwd -P)
trap 'chmod -R u+rwX "$work" 2> /dev/null; rm -rf "$work"' EXIT
arch=$(uname -m)
commit=0123456789abcdef0123456789abcdef01234567

repo=$work/repo
mkdir -p "$repo/scripts/release" "$repo/apps/desktop/dist"
cp "$here"/*.sh "$repo/scripts/release/"
echo '<!doctype html><title>Klipolo</title>' > "$repo/apps/desktop/dist/index.html"
check=$repo/scripts/release/check-bundle.sh
# The checkout's files the Cap MIT check reads: the app's own Cap-MIT.txt (a bundle's copy must have its licence text)
# and the capture crate's four files from Cap's MIT crates, which that copy's first line names.
source_file() {  # source_file <path in the test's tree> <header line>...
  local f=$repo/$1
  shift
  mkdir -p "$(dirname "$f")"
  printf '%s\n' "$@" 'pub fn f() {}' > "$f"
}
mkdir -p "$repo/crates/klipolo-app/licenses"
cp "$app_crate/licenses/Cap-MIT.txt" "$repo/crates/klipolo-app/licenses/"
capture=crates/klipolo-capture/src
cap_mit="// MIT (Cap's \`scap-screencapturekit\`), Copyright (c) 2023-present Cap Software, Inc."
for f in vendor/cap/sck/capture.rs vendor/cap/sck/config.rs vendor/cap/sck/mod.rs platform/macos/camera.rs; do
  source_file "$capture/$f" '// SPDX-License-Identifier: MIT' "$cap_mit"
done

# The two Mach-O files, built once; the dylib's install name is @rpath/libfoo.1.dylib.
build=$work/build
mkdir -p "$build"
printf 'int foo(void) { return 1; }\n' > "$build/foo.c"
printf 'int foo(void);\nint main(void) { return foo() - 1; }\n' > "$build/main.c"
cc=(xcrun clang -arch "$arch" -mmacosx-version-min=15.0 -Wl,-headerpad_max_install_names)
(cd "$build" && "${cc[@]}" -dynamiclib -install_name @rpath/libfoo.1.dylib -o libfoo.1.dylib foo.c)
(cd "$build" && "${cc[@]}" -o klipolo-app main.c libfoo.1.dylib -Wl,-rpath,@executable_path/../Frameworks)

# codesign as the bundle check calls it, so no case signs anything:
# `codesign -d --entitlements - --xml <dir>/Klipolo.app` prints <dir>/signature.plist, the entitlements the case's
# bundle is taken to be signed with, and fails as codesign does for an unsigned app when there is none. Any other call
# fails.
fakes=$work/fakes
mkdir -p "$fakes"
cat > "$fakes/codesign" << 'EOF'
#!/usr/bin/env bash
[ $# -eq 5 ] && [ "$1 $2 $3 $4" = "-d --entitlements - --xml" ] \
  || { echo "fake codesign: unexpected call: $*" >&2; exit 64; }
signature=$(dirname "$5")/signature.plist
[ -f "$signature" ] || { echo "$5: code object is not signed at all" >&2; exit 1; }
cat "$signature"
EOF
chmod +x "$fakes/codesign"
PATH=$fakes:$PATH

# A bundle that passes, at <dir>/Klipolo.app, signed (see the fake codesign) with the app's entitlements.
make_app() {
  local c=$1/Klipolo.app/Contents
  mkdir -p "$c/MacOS" "$c/Frameworks" "$c/Resources/licenses" "$c/Resources/fonts" "$c/Resources/library"
  # The app's own Info.plist keys (the permission strings), and the two the build adds that the checks read.
  cp "$app_crate/Info.plist" "$c/Info.plist"
  /usr/libexec/PlistBuddy -c 'Add :CFBundleExecutable string klipolo-app' \
    -c 'Add :CFBundleIdentifier string com.klipolo.app' "$c/Info.plist" > /dev/null
  cp "$app_crate/entitlements.plist" "$1/signature.plist"
  cp "$build/klipolo-app" "$c/MacOS/"
  cp "$build/libfoo.1.dylib" "$c/Frameworks/"
  for f in FFmpeg-LGPL.txt COPYING.LGPLv2.1 LICENSE.txt Archivo-OFL.txt; do
    echo "$f" > "$c/Resources/licenses/$f"
  done
  cp "$app_crate/licenses/Cap-MIT.txt" "$c/Resources/licenses/"
  printf 'Klipolo: third-party notices\n\nMIT\n' > "$c/Resources/licenses/THIRD-PARTY-NOTICES.txt"
  printf 'Klipolo\nSource code: https://github.com/owner/klipolo (commit %s)\n' "$commit" \
    > "$c/Resources/licenses/NOTICE.txt"
  echo 'OFL' > "$c/Resources/fonts/OFL.txt"
  echo '{"items": []}' > "$c/Resources/library/manifest.json"
  printf 'Klipolo pack\n\nApache License\nVersion 2.0, January 2004\n' > "$c/Resources/library/LICENSE.txt"
}

passed=0 failed=0
# expect <name> pass <command...>, or expect <name> fail <text the output must hold> <command...>: a failure is exit 1.
expect() {
  local name=$1 want=$2 text="" out rc
  shift 2
  if [ "$want" = fail ]; then text=$1; shift; fi
  out=$("$@" 2>&1) && rc=0 || rc=$?
  if { [ "$want" = pass ] && [ "$rc" -eq 0 ]; } || { [ "$want" = fail ] && [ "$rc" -eq 1 ] && grep -qF -- "$text" <<< "$out"; }; then
    echo "ok    $name"
    passed=$((passed + 1))
  else
    echo "FAIL  $name: wanted $want${text:+ with \"$text\"}, got exit $rc:"
    sed 's/^/        /' <<< "$out"
    failed=$((failed + 1))
  fi
}
# A fresh good bundle for one case: case_app <name> prints its path.
case_app() {
  make_app "$work/$1"
  echo "$work/$1/Klipolo.app"
}

app=$(case_app good)
expect "a good bundle passes" pass "$check" "$app" "$arch"

app=$(case_app no-ffmpeg-notice)
rm "$app/Contents/Resources/licenses/FFmpeg-LGPL.txt"
expect "a missing licence fails" fail "licenses/FFmpeg-LGPL.txt isn't in the app" "$check" "$app" "$arch"

app=$(case_app no-third-party)
rm "$app/Contents/Resources/licenses/THIRD-PARTY-NOTICES.txt"
expect "no third-party notices fails" fail "licenses/THIRD-PARTY-NOTICES.txt isn't in the app" "$check" "$app" "$arch"

app=$(case_app empty-third-party)
: > "$app/Contents/Resources/licenses/THIRD-PARTY-NOTICES.txt"
expect "empty third-party notices fail" fail "licenses/THIRD-PARTY-NOTICES.txt is empty" "$check" "$app" "$arch"

app=$(case_app stub-third-party)
printf 'x' > "$app/Contents/Resources/licenses/THIRD-PARTY-NOTICES.txt"
expect "1-byte third-party notices fail" fail "doesn't start 'Klipolo: third-party notices'" "$check" "$app" "$arch"

app=$(case_app no-pack)
rm -r "$app/Contents/Resources/library"
expect "no bundled pack fails" fail "the bundled pack isn't in the app" "$check" "$app" "$arch"

app=$(case_app no-pack-licence)
rm "$app/Contents/Resources/library/LICENSE.txt"
expect "no pack licence fails" fail "no Resources/library/LICENSE.txt" "$check" "$app" "$arch"

app=$(case_app pack-licence-without-apache)
echo 'Klipolo pack' > "$app/Contents/Resources/library/LICENSE.txt"
expect "a pack licence without the Apache text fails" fail "has no Apache License text" "$check" "$app" "$arch"

app=$(case_app no-archivo)
rm "$app/Contents/Resources/licenses/Archivo-OFL.txt"
expect "no Archivo licence fails" fail "licenses/Archivo-OFL.txt isn't in the app" "$check" "$app" "$arch"

# The permission prompts' words: the good bundle has crates/klipolo-app/Info.plist's three, and a bundle without one, or
# with one empty, fails naming it.
for key in NSScreenCaptureUsageDescription NSMicrophoneUsageDescription NSCameraUsageDescription; do
  app=$(case_app "no-$key")
  /usr/libexec/PlistBuddy -c "Delete :$key" "$app/Contents/Info.plist"
  expect "a bundle without $key fails" fail "Info.plist has no $key" "$check" "$app" "$arch"
done
app=$(case_app empty-camera-words)
plutil -replace NSCameraUsageDescription -string '' "$app/Contents/Info.plist"
expect "an empty NSCameraUsageDescription fails" fail "Info.plist has no NSCameraUsageDescription" "$check" "$app" "$arch"

# The camera and microphone entitlements, read from the bundle's signature (the fake codesign): the good bundle is
# signed with crates/klipolo-app/entitlements.plist. An unsigned bundle fails, and so does a signature without either
# key, or with one false.
app=$(case_app unsigned)
rm "$work/unsigned/signature.plist"
expect "an unsigned bundle fails with codesign's words" fail "codesign says: $app: code object is not signed at all" \
  "$check" "$app" "$arch"
app=$(case_app no-entitlements)
: > "$work/no-entitlements/signature.plist"
expect "a signature with no entitlements fails" fail "the app's signature has no entitlements" "$check" "$app" "$arch"
for key in com.apple.security.device.camera com.apple.security.device.audio-input; do
  app=$(case_app "no-$key")
  /usr/libexec/PlistBuddy -c "Delete :$key" "$work/no-$key/signature.plist"
  expect "a signature without $key fails" fail "the app's entitlements don't grant $key" "$check" "$app" "$arch"
done
app=$(case_app camera-false)
/usr/libexec/PlistBuddy -c 'Set :com.apple.security.device.camera false' "$work/camera-false/signature.plist"
expect "a signature with the camera entitlement false fails" fail \
  "the app's entitlements don't grant com.apple.security.device.camera" "$check" "$app" "$arch"

# Cap's MIT licence ships with the checkout's copy's licence text, and its first line names exactly the checkout's files
# from Cap's MIT crates: each starts with the SPDX MIT line and names Cap Software, Inc. in its header. Besides the
# capture crate's four, the tree gets an AGPL file from Cap and another author's MIT file, which it needn't name.
app=$(case_app no-cap-mit)
rm "$app/Contents/Resources/licenses/Cap-MIT.txt"
expect "no Cap MIT licence fails" fail "licenses/Cap-MIT.txt isn't in the app" "$check" "$app" "$arch"
source_file "$capture/hosttime.rs" '// Vendored from Cap (https://github.com/CapSoftware/Cap)' \
  '// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.'
source_file crates/klipolo-other/src/lib.rs '// SPDX-License-Identifier: MIT' '// Copyright (c) 2026 Someone Else'
app=$(case_app cap-mit)
expect "the app's Cap-MIT.txt names the capture crate's MIT files" pass "$check" "$app" "$arch"
source_file "$capture/vendor/cap/sck/stream.rs" '// SPDX-License-Identifier: MIT' "$cap_mit"
expect "a Cap MIT file Cap-MIT.txt doesn't name fails" fail \
  "Cap-MIT.txt's first line doesn't name $capture/vendor/cap/sck/stream.rs" "$check" "$app" "$arch"
echo "Also $capture/vendor/cap/sck/stream.rs." >> "$app/Contents/Resources/licenses/Cap-MIT.txt"
expect "a Cap MIT file named below the first line fails" fail \
  "Cap-MIT.txt's first line doesn't name $capture/vendor/cap/sck/stream.rs" "$check" "$app" "$arch"
rm "$repo/$capture/vendor/cap/sck/stream.rs"
# A name left over from a file renamed or gone fails, and so does a near miss, which names no file.
app=$(case_app cap-mit-stale)
sed -i '' "1s|\$| Also $capture/vendor/cap/sck/old.rs.|" "$app/Contents/Resources/licenses/Cap-MIT.txt"
expect "a name on Cap-MIT.txt's first line that isn't a Cap MIT file fails" fail \
  "first line names $capture/vendor/cap/sck/old.rs, which isn't a file from Cap's MIT crates" "$check" "$app" "$arch"
app=$(case_app cap-mit-near-miss)
sed -i '' '1s|/sck/mod\.rs |/sck/mod.rs.bak |' "$app/Contents/Resources/licenses/Cap-MIT.txt"
expect "mod.rs.bak on the first line doesn't name mod.rs" fail \
  "Cap-MIT.txt's first line doesn't name $capture/vendor/cap/sck/mod.rs " "$check" "$app" "$arch"
# The licence text below the first line is the checkout's copy's, all of it.
app=$(case_app cap-mit-first-line-only)
head -n 1 "$app_crate/licenses/Cap-MIT.txt" > "$app/Contents/Resources/licenses/Cap-MIT.txt"
expect "a Cap-MIT.txt without the licence text fails" fail \
  "licenses/Cap-MIT.txt's licence text (below its first line) isn't the checkout's" "$check" "$app" "$arch"
app=$(case_app cap-mit-truncated)
sed -i '' '/^THE SOFTWARE IS PROVIDED/,$d' "$app/Contents/Resources/licenses/Cap-MIT.txt"
expect "a Cap-MIT.txt without the licence's disclaimer fails" fail \
  "licenses/Cap-MIT.txt's licence text (below its first line) isn't the checkout's" "$check" "$app" "$arch"
# A source file that can't be read fails rather than be skipped.
app=$(case_app cap-mit-unreadable)
chmod 000 "$repo/$capture/hosttime.rs"
expect "an unreadable source file fails the Cap MIT check" fail \
  "couldn't read every source file under crates/ to look for files from Cap's MIT crates" "$check" "$app" "$arch"
chmod 644 "$repo/$capture/hosttime.rs"
# The Linux gate's form (scripts/ci-linux.sh): the checkout's own copy against its files.
own_cap_checked() { (. "$repo/scripts/release/lib.sh" && cap_mit_check "$repo" "$repo/$1" "$1"); }
expect "the Linux gate's Cap MIT check passes the checkout's copy" pass \
  own_cap_checked crates/klipolo-app/licenses/Cap-MIT.txt
source_file "$capture/vendor/cap/sck/stream.rs" '// SPDX-License-Identifier: MIT' "$cap_mit"
expect "the Linux gate's Cap MIT check fails a file its copy doesn't name" fail \
  "crates/klipolo-app/licenses/Cap-MIT.txt's first line doesn't name $capture/vendor/cap/sck/stream.rs" \
  own_cap_checked crates/klipolo-app/licenses/Cap-MIT.txt
rm "$repo/$capture/vendor/cap/sck/stream.rs"

# NOTICE.txt's source code line: a placeholder left fails, and so does a notice without the address or the full commit.
app=$(case_app source-placeholder)
notice_in_app=$app/Contents/Resources/licenses/NOTICE.txt
cp "$here/../../crates/klipolo-app/licenses/NOTICE.txt" "$notice_in_app"
expect "NOTICE.txt with the placeholders fails" fail "still has a {{SOURCE_URL}} or {{SOURCE_COMMIT}} placeholder" \
  "$check" "$app" "$arch"
sed -i '' 's|{{SOURCE_URL}}|https://github.com/owner/klipolo|' "$notice_in_app"
expect "NOTICE.txt with the commit placeholder left fails" fail "still has a {{SOURCE_URL}} or {{SOURCE_COMMIT}}" \
  "$check" "$app" "$arch"
cp "$here/../../crates/klipolo-app/licenses/NOTICE.txt" "$notice_in_app"
(. "$repo/scripts/release/lib.sh" && fill_source "$notice_in_app" https://github.com/owner/klipolo "$commit")
expect "NOTICE.txt with the address and commit filled in passes" pass "$check" "$app" "$arch"
sed -i '' "s|$commit|${commit:0:7}|" "$notice_in_app"
expect "NOTICE.txt with a short commit fails" fail "gives no source code line" "$check" "$app" "$arch"

app=$(case_app no-source-line)
echo 'Klipolo' > "$app/Contents/Resources/licenses/NOTICE.txt"
expect "NOTICE.txt without the source code line fails" fail "gives no source code line" "$check" "$app" "$arch"

app=$(case_app no-rpath)
install_name_tool -delete_rpath @executable_path/../Frameworks "$app/Contents/MacOS/klipolo-app" 2> /dev/null
expect "no Frameworks rpath fails" fail "has no @executable_path/../Frameworks LC_RPATH" "$check" "$app" "$arch"

# The rpath is compared as text, not as a pattern: `.` would match any character.
app=$(case_app rpath-lookalike)
install_name_tool -rpath @executable_path/../Frameworks @executable_path/xx/Frameworks "$app/Contents/MacOS/klipolo-app" 2> /dev/null
expect "a look-alike Frameworks rpath fails" fail "has no @executable_path/../Frameworks LC_RPATH" "$check" "$app" "$arch"

# The main executable's rpath is checked even when the walk over the files doesn't meet it (here it is a link to a
# Mach-O elsewhere in the bundle that has no rpath).
app=$(case_app main-is-a-link)
mkdir "$app/Contents/Resources/bin"
mv "$app/Contents/MacOS/klipolo-app" "$app/Contents/Resources/bin/"
install_name_tool -delete_rpath @executable_path/../Frameworks "$app/Contents/Resources/bin/klipolo-app" 2> /dev/null
ln -s ../Resources/bin/klipolo-app "$app/Contents/MacOS/klipolo-app"
expect "a main executable the walk skips is still checked" fail "has no @executable_path/../Frameworks LC_RPATH" \
  "$check" "$app" "$arch"

app=$(case_app no-dylib)
rm "$app/Contents/Frameworks/libfoo.1.dylib"
expect "a missing @rpath library fails" fail "Contents/Frameworks/libfoo.1.dylib isn't there" "$check" "$app" "$arch"

# The source code address may name the builder's own account (github.com/<user>/klipolo): NOTICE.txt passes.
app=$(case_app source-names-the-user)
printf 'Klipolo\nSource code: https://github.com/%s/klipolo (commit %s)\n' "$(id -un)" "$commit" \
  > "$app/Contents/Resources/licenses/NOTICE.txt"
expect "a source address naming the builder's account passes" pass "$check" "$app" "$arch"

app=$(case_app home-in-resources)
echo "built in $HOME/klipolo" > "$app/Contents/Resources/fonts/note.txt"
expect "a resource naming the home folder fails" fail "Resources/fonts/note.txt" "$check" "$app" "$arch"

# The page is in the binary compressed, so its build output is searched where pnpm build wrote it.
app=$(case_app home-in-page)
mkdir -p "$repo/apps/desktop/dist/assets"
echo "const at = '$HOME/klipolo/src/main.tsx';" > "$repo/apps/desktop/dist/assets/index.js"
expect "the page's build naming the home folder fails" fail "apps/desktop/dist/assets/index.js" "$check" "$app" "$arch"
rm -r "$repo/apps/desktop/dist/assets"
mv "$repo/apps/desktop/dist" "$repo/apps/desktop/dist.away"
expect "no page build fails" fail "no page build at $repo/apps/desktop/dist" "$check" "$app" "$arch"
mv "$repo/apps/desktop/dist.away" "$repo/apps/desktop/dist"
expect "the page's build without the home folder passes" pass "$check" "$app" "$arch"
# A page build kept elsewhere (an app built earlier) is given as the third argument.
kept=$work/kept-page
mkdir -p "$kept/assets"
echo '<!doctype html>' > "$kept/index.html"
echo "const at = '$HOME/klipolo/src/main.tsx';" > "$kept/assets/index.js"
expect "a page build given that names the home folder fails" fail "kept-page/assets/index.js" "$check" "$app" "$arch" "$kept"
rm "$kept/assets/index.js"
expect "a page build given without the home folder passes" pass "$check" "$app" "$arch" "$kept"
expect "a page build given that isn't there fails" fail "no page build at $work/nowhere" "$check" "$app" "$arch" "$work/nowhere"

# A file grep can't read fails the check, and doesn't hide a file that names the home folder.
app=$(case_app unreadable)
chmod 000 "$app/Contents/Resources/licenses/NOTICE.txt"
expect "an unreadable file fails" fail "couldn't read every file" "$check" "$app" "$arch"
echo "built in $HOME/klipolo" > "$app/Contents/Resources/fonts/note.txt"
expect "an unreadable file doesn't hide one naming the home folder" fail "Resources/fonts/note.txt" "$check" "$app" "$arch"

# The user name, through lib.sh's naming_files with fixed names (whoever runs the test): only as a whole path part.
names=$work/names
mkdir -p "$names/long-word" "$names/long-path" "$names/plain" "$names/path"
echo 'xklipolobuildery klipolobuilders klipolobuilder.txt' > "$names/long-word/a.txt"
echo 'built in /Volumes/klipolobuilder/klipolo' > "$names/long-path/b.txt"
# An account with a common name: only /test/ counts, not "attest" or "testing" or "/tests/".
echo 'attest testing contest /tests/ test.txt' > "$names/plain/a.txt"
echo 'built in /Volumes/test/klipolo' > "$names/path/b.txt"
found() {  # found <user> <dir> <expected output>: naming_files' output, with grep's status 0 or 1 to match
  local out rc
  out=$(. "$repo/scripts/release/lib.sh" && naming_files /no/such/home "$1" "$2") && rc=0 || rc=$?
  [ "$out" = "$3" ] && [ "$rc" -eq "$([ -n "$3" ] && echo 0 || echo 1)" ]
}
expect "a long user name is found as a path part" pass found klipolobuilder "$names/long-path" "$names/long-path/b.txt"
expect "a long user name inside longer words isn't found" pass found klipolobuilder "$names/long-word" ""
expect "a user named test isn't found in words" pass found test "$names/plain" ""
expect "a user named test is found as a path part" pass found test "$names/path" "$names/path/b.txt"
expect "a user name under 4 characters isn't looked for" pass found lib "$names/path" ""
# The source code address NOTICE.txt gives may name the account that owns the code (github.com/<user>/...) when it is
# also the builder's user name: that address is allowed, the user name anywhere else still counts.
mkdir -p "$names/notice" "$names/notice-and-path"
echo 'Source code: https://github.com/klipolobuilder/klipolo (commit abc)' > "$names/notice/NOTICE.txt"
printf 'Source code: https://github.com/klipolobuilder/klipolo (commit abc)\nbuilt in /Volumes/klipolobuilder/klipolo\n' \
  > "$names/notice-and-path/NOTICE.txt"
found_allowing() {  # found_allowing <user> <allowed text> <dir> <expected output>
  local out rc
  out=$(. "$repo/scripts/release/lib.sh" && naming_files /no/such/home "$1" --allow "$2" "$3") && rc=0 || rc=$?
  [ "$out" = "$4" ] && [ "$rc" -eq "$([ -n "$4" ] && echo 0 || echo 1)" ]
}
expect "a user name only in the allowed source address isn't found" pass \
  found_allowing klipolobuilder https://github.com/klipolobuilder/klipolo "$names/notice" ""
expect "a user name outside the allowed source address is still found" pass \
  found_allowing klipolobuilder https://github.com/klipolobuilder/klipolo "$names/notice-and-path" \
  "$names/notice-and-path/NOTICE.txt"

# The source code address build-mac.sh puts in NOTICE.txt: https:// with plain URL characters only.
url_ok() { (. "$repo/scripts/release/lib.sh" && is_source_url "$1"); }
url_refused() { ! url_ok "$1"; }
for u in https://github.com/owner/klipolo https://klipolo.app/source https://gitlab.com/o/klipolo/-/tree/v0.1.0; do
  expect "source address $u is taken" pass url_ok "$u"
done
for u in "" http://klipolo.app/source "https://" "https://klipolo.app/a b" "https://klipolo.app/?a=1&b=2" \
  'https://klipolo.app/a"b' 'https://klipolo.app/a\b' "ftp://klipolo.app/source"; do
  expect "source address \"$u\" is refused" pass url_refused "$u"
done
notice=$work/NOTICE.txt
filled() {  # filled <url>: the checkout's NOTICE.txt, filled in, has one source code line with the address and commit
  cp "$here/../../crates/klipolo-app/licenses/NOTICE.txt" "$notice"
  (. "$repo/scripts/release/lib.sh" && fill_source "$notice" "$1" "$commit") || return 1
  [ "$(grep -c "^Source code: $1 (commit $commit)\$" "$notice")" = 1 ] && ! grep -qF '{{SOURCE_' "$notice"
}
expect "NOTICE.txt gets the source address and commit" pass filled https://github.com/owner/klipolo
unchanged() {  # unchanged <url> <commit>: a refused address or commit leaves NOTICE.txt as it was
  cp "$here/../../crates/klipolo-app/licenses/NOTICE.txt" "$notice"
  ! (. "$repo/scripts/release/lib.sh" && fill_source "$notice" "$1" "$2" 2> /dev/null) \
    && cmp -s "$notice" "$here/../../crates/klipolo-app/licenses/NOTICE.txt"
}
expect "a refused address leaves NOTICE.txt alone" pass unchanged "https://klipolo.app/?a=1&b=2" "$commit"
expect "a short commit leaves NOTICE.txt alone" pass unchanged https://github.com/owner/klipolo "${commit:0:7}"
# build-mac.sh refuses to start without the address (the copy in the test's tree, which has no FFmpeg to build with).
refused_build() {  # refused_build <KLIPOLO_SOURCE_URL or "unset">
  local out rc
  if [ "$1" = unset ]; then
    out=$(env -u KLIPOLO_SOURCE_URL "$repo/scripts/release/build-mac.sh" x86_64 0.1.0 2>&1) && rc=0 || rc=$?
  else
    out=$(KLIPOLO_SOURCE_URL=$1 "$repo/scripts/release/build-mac.sh" x86_64 0.1.0 2>&1) && rc=0 || rc=$?
  fi
  [ "$rc" -eq 2 ] && grep -qF 'refused: set KLIPOLO_SOURCE_URL' <<< "$out"
}
expect "build-mac.sh refuses to start without KLIPOLO_SOURCE_URL" pass refused_build unset
# build-mac.sh refuses a copy with no free credit, unless KLIPOLO_NO_TRIAL=1 asks for one.
trial_refused() {  # trial_refused <KLIPOLO_NO_TRIAL>: whether the build stopped for the missing KLIPOLO_TRIAL_CODE
  local out
  out=$(env -u KLIPOLO_TRIAL_CODE KLIPOLO_NO_TRIAL="$1" KLIPOLO_SOURCE_URL=https://github.com/owner/klipolo \
    "$repo/scripts/release/build-mac.sh" x86_64 0.1.0 2>&1)
  grep -qF 'refused: set KLIPOLO_TRIAL_CODE' <<< "$out"
}
trial_not_refused() { ! trial_refused 1; }
expect "build-mac.sh refuses to start without KLIPOLO_TRIAL_CODE" pass trial_refused ""
expect "build-mac.sh builds without free credit when KLIPOLO_NO_TRIAL=1" pass trial_not_refused
# build-mac.sh checks the third-party notices are current once the page's packages are in, before the app is built.
notices_checked() {
  awk '/^pnpm install --frozen-lockfile$/ && !i {i = NR} /third-party\.sh" --check$/ && !c {c = NR}
    /^  pnpm tauri build --ci / && !b {b = NR} END {exit !(i && c && b && i < c && c < b)}' "$here/build-mac.sh"
}
expect "build-mac.sh checks the notices between pnpm install and the build" pass notices_checked
expect "build-mac.sh refuses an http:// KLIPOLO_SOURCE_URL" pass refused_build http://klipolo.app/source

# sign.sh --list prints paths inside the app, whatever characters the app's own path has (it signs nothing). It runs
# from this folder, as it reads the checkout's entitlements.
odd="$work/odd|name [1] & co.v2"
mkdir -p "$odd"
make_app "$odd"
listed() {  # listed <app> <expected output>
  local out
  out=$("$here/sign.sh" --list "$1") && [ "$out" = "$2" ]
}
expect "sign.sh --list prints paths inside an app under an odd path" pass \
  listed "$odd/Klipolo.app" "Contents/Frameworks/libfoo.1.dylib"

echo
echo "$passed passed, $failed failed"
[ "$failed" -eq 0 ]
