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
  for f in FFmpeg-LGPL.txt COPYING.LGPLv2.1 LICENSE.txt NOTICE.txt; do echo "$f" > "$c/Resources/licenses/$f"; done
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

app=$(case_app no-rpath)
install_name_tool -delete_rpath @executable_path/../Frameworks "$app/Contents/MacOS/klipolo-app" 2> /dev/null
expect "no Frameworks rpath fails" fail "has no @executable_path/../Frameworks LC_RPATH" "$check" "$app" "$arch"

app=$(case_app no-dylib)
rm "$app/Contents/Frameworks/libfoo.1.dylib"
expect "a missing @rpath library fails" fail "Contents/Frameworks/libfoo.1.dylib isn't there" "$check" "$app" "$arch"

app=$(case_app home-in-resources)
echo "built in $HOME/klipolo" > "$app/Contents/Resources/fonts/note.txt"
expect "a resource naming the home folder fails" fail "Resources/fonts/note.txt" "$check" "$app" "$arch"

echo
echo "$passed passed, $failed failed"
[ "$failed" -eq 0 ]
