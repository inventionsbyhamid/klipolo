#!/usr/bin/env bash
# The release checks' own tests, on a small app bundle made here (macOS with the Command Line Tools' clang; nothing is
# signed, nothing reaches the network, and it takes seconds):
#   scripts/release/release.test.sh
# The bundle is an executable that loads one dylib through @rpath, with the licences and a fonts folder: check-bundle.sh
# must pass it. Then each broken copy must fail with its own message. check-bundle.sh runs from a copy of this folder
# in a checkout-shaped tree, so the test owns the page's build output it also searches.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
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

# The two Mach-O files, built once; the dylib's install name is @rpath/libfoo.1.dylib.
build=$work/build
mkdir -p "$build"
printf 'int foo(void) { return 1; }\n' > "$build/foo.c"
printf 'int foo(void);\nint main(void) { return foo() - 1; }\n' > "$build/main.c"
cc=(xcrun clang -arch "$arch" -mmacosx-version-min=15.0 -Wl,-headerpad_max_install_names)
(cd "$build" && "${cc[@]}" -dynamiclib -install_name @rpath/libfoo.1.dylib -o libfoo.1.dylib foo.c)
(cd "$build" && "${cc[@]}" -o klipolo-app main.c libfoo.1.dylib -Wl,-rpath,@executable_path/../Frameworks)

# A bundle that passes, at <dir>/Klipolo.app.
make_app() {
  local c=$1/Klipolo.app/Contents
  mkdir -p "$c/MacOS" "$c/Frameworks" "$c/Resources/licenses" "$c/Resources/fonts"
  cat > "$c/Info.plist" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>klipolo-app</string>
<key>CFBundleIdentifier</key><string>com.klipolo.app</string>
</dict></plist>
EOF
  cp "$build/klipolo-app" "$c/MacOS/"
  cp "$build/libfoo.1.dylib" "$c/Frameworks/"
  for f in FFmpeg-LGPL.txt COPYING.LGPLv2.1 LICENSE.txt Archivo-OFL.txt; do
    echo "$f" > "$c/Resources/licenses/$f"
  done
  printf 'Klipolo: third-party notices\n\nMIT\n' > "$c/Resources/licenses/THIRD-PARTY-NOTICES.txt"
  printf 'Klipolo\nSource code: https://github.com/owner/klipolo (commit %s)\n' "$commit" \
    > "$c/Resources/licenses/NOTICE.txt"
  echo 'OFL' > "$c/Resources/fonts/OFL.txt"
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

app=$(case_app no-archivo)
rm "$app/Contents/Resources/licenses/Archivo-OFL.txt"
expect "no Archivo licence fails" fail "licenses/Archivo-OFL.txt isn't in the app" "$check" "$app" "$arch"

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
