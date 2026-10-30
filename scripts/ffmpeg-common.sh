# Shared by scripts/build-ffmpeg.sh (macOS) and scripts/build-ffmpeg-linux.sh: the pinned LGPL FFmpeg source, its
# checks, and the one configure flag list (each OS adds only its own hardware and loader flags). Sourced, not run.
# Never add --enable-gpl, --enable-nonfree or --enable-version3: the verify step fails the build if any is on.
VERSION=9.0.2
SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
# "FFmpeg release signing key <ffmpeg-devel@ffmpeg.org>"; pinned, so a swapped key file can't pass.
KEY_FPR=FCF986EA15E6E293A5644F10B4322F04D67658D8
PREFIX=/opt/klipolo-ffmpeg

# Sets FLAGS for `macos` or `linux`: only the libraries Klipolo links (no programs, no libavdevice), nothing
# autodetected. The macOS order is the configure line in the app's FFmpeg-LGPL.txt (scripts/release/build-mac.sh).
ffmpeg_flags() {
  FLAGS=(--enable-shared --disable-static --disable-programs --disable-doc --disable-debug --disable-autodetect)
  if [ "$1" = macos ]; then FLAGS+=(--enable-videotoolbox --enable-audiotoolbox); fi
  FLAGS+=(--enable-zlib --disable-indevs --disable-outdevs --disable-avdevice)
  case "$1" in
    macos) FLAGS+=(--install-name-dir=@rpath --extra-cflags=-mmacosx-version-min=15.0
             --extra-ldflags=-mmacosx-version-min=15.0) ;;
    linux) ;;  # RUNPATH $ORIGIN is set after install (patchelf), so make never sees a `$`.
    *) echo "ffmpeg_flags: unknown OS $1" >&2; return 1 ;;
  esac
}

sha256_ok() {  # <file> <sha256>
  if command -v sha256sum >/dev/null; then echo "$2  $1" | sha256sum -c - >/dev/null
  else echo "$2  $1" | shasum -a 256 -c - >/dev/null; fi
}

# Puts the verified tarball at $tarball (in <build-dir>/dl): reused or downloaded; SHA-256 always, GPG when gpg and
# the key file are here. A tarball that fails either check is deleted, so the next run downloads it again. Sets G to
# the throwaway keyring (the caller's cleanup removes it).
fetch_verified_source() {  # <build-dir>
  tarball="$1/dl/ffmpeg-$VERSION.tar.xz"
  mkdir -p "$1/dl"
  fetch() { curl -fsSL --proto '=https' --tlsv1.2 -o "$2.part" "$1" && mv "$2.part" "$2"; }
  reject() { rm -f "$tarball" "$tarball.part"; echo "$1; deleted $tarball" >&2; exit 1; }
  [ -f "$tarball" ] || fetch "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz" "$tarball"
  sha256_ok "$tarball" "$SHA256" || reject "SHA-256 mismatch"
  echo "SHA-256 OK: $SHA256"
  [ -f "$tarball.asc" ] || fetch "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz.asc" "$tarball.asc" || true
  [ -f "$1/dl/ffmpeg-devel.asc" ] || fetch "https://ffmpeg.org/ffmpeg-devel.asc" "$1/dl/ffmpeg-devel.asc" || true
  if command -v gpg >/dev/null && [ -f "$tarball.asc" ] && [ -f "$1/dl/ffmpeg-devel.asc" ]; then
    G="$(mktemp -d)"
    gpg --homedir "$G" --batch --quiet --import "$1/dl/ffmpeg-devel.asc" 2>/dev/null ||
      { echo "The FFmpeg key file couldn't be imported: $1/dl/ffmpeg-devel.asc" >&2; exit 1; }
    status=$(gpg --homedir "$G" --batch --status-fd 1 --verify "$tarball.asc" "$tarball" 2>/dev/null || true)
    grep -q "^\[GNUPG:\] VALIDSIG $KEY_FPR " <<<"$status" || reject "GPG signature not valid for $KEY_FPR"
    echo "GPG OK: signed by $KEY_FPR"
  else
    echo "GPG not checked (gpg, the signature or the key file is missing); SHA-256 only"
  fi
}
