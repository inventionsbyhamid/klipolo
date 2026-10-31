# Klipolo

Klipolo is a Mac video editor you talk to. Drop in raw footage, say what you
want in a chat box, and it edits. Anything that removes footage comes back as a
proposal you approve, with the removed parts shown in red on the timeline.

It runs on Apple Silicon and Intel Macs and is free while in beta.

## Repo layout

```
apps/desktop/    The app's page (React, Vite)
crates/          The Rust crates; klipolo-app is the Tauri app
scripts/         FFmpeg, Linux gate and release scripts
site/            Landing page (static, hand-written, no build step)
  index.html     The whole page
  styles.css     All styles, light and dark mode
  assets/        Icon, favicon and self-hosted fonts
```

## Build the app on a Mac

You need macOS 15, the Xcode Command Line Tools, rustup (it installs the
Rust version in `rust-toolchain.toml`), Node 22 and pnpm 11.

1. FFmpeg. Klipolo links its own LGPL build of FFmpeg 9.0.2, never
   Homebrew's. Point this checkout at a shared build, or build one:

   ```sh
   scripts/link-ffmpeg.sh            # uses the main checkout's build
   scripts/build-ffmpeg.sh           # or builds this Mac's arch
   scripts/build-ffmpeg.sh --arch arm64
   ```

   `.cargo/config.toml` points at the Intel prefix. On Apple Silicon, point
   the three FFmpeg variables at the arm64 prefix:

   ```sh
   export FFMPEG_DIR=$PWD/.build/ffmpeg-9.0/arm64/prefix
   export KLIPOLO_FFMPEG_PREFIX=$FFMPEG_DIR PKG_CONFIG_LIBDIR=$FFMPEG_DIR/lib/pkgconfig
   ```

2. Install the page's packages and run the app:

   ```sh
   pnpm install
   cd apps/desktop
   lockf -k /tmp/klipolo-mac-build.lock pnpm tauri dev
   ```

The Mac build lock keeps it to one heavy build at a time. Wrap every cargo
build or test, `pnpm build`, `pnpm tauri dev` and release script in
`lockf -k /tmp/klipolo-mac-build.lock`. It waits for whoever holds it.

## Test on Linux

The Linux gate, `scripts/ci-linux.sh`, runs rustfmt, clippy, the Rust tests,
the GPU tests on Mesa lavapipe, the page's typecheck and tests, and checks that
the bindings and the third-party notices are current. Run it on a Linux machine
with Docker, in the `klipolo-ci` image.

Copy the checkout to that machine (`klipolo-copy` here). From the copy, build
the image once, then run the gate:

```sh
cd klipolo-copy
docker build -f ci/linux.Dockerfile -t klipolo-ci ci
sudo chown 1000:1000 . && sudo find . -mindepth 1 -maxdepth 1 ! -name target ! -name .build \
  -exec chown -R 1000:1000 {} +
docker run --rm --name klipolo-gate --cpus 16 --memory 24g \
  -v "$PWD":/src -w /src klipolo-ci scripts/ci-linux.sh
```

The image runs as uid 1000, so the copy must belong to it, and `chown` needs
sudo unless you are root. The caps keep the gate from taking the whole
machine; lower them on a smaller one.

## Cut a release

Both arches build on the Intel Mac (Apple Silicon is cross-compiled). From a
clean checkout of the release commit:

```sh
export KLIPOLO_SOURCE_URL=https://...    # the public source code address
lockf -k /tmp/klipolo-mac-build.lock nice -n 10 scripts/release/build-mac.sh x86_64 0.1.0
lockf -k /tmp/klipolo-mac-build.lock nice -n 10 scripts/release/build-mac.sh aarch64 0.1.0
```

Each writes `dist/Klipolo-<version>-<arch>.dmg`. `build-mac.sh` refuses to
start without `KLIPOLO_SOURCE_URL`. The app's `NOTICE.txt` gives that address
and the commit built, as the source code of this version. It then runs these
steps in order and stops at the first failure. Each also runs on its own:

- `scripts/release/check-bundle.sh <app> <x86_64|arm64> [<page build>]`
  checks the bundle before anything is signed: the licences (with the
  third-party notices, the source address and the commit), one arch, macOS
  15.0, load paths and rpaths, and no file naming your home folder, in the app
  or in the page build. That is `apps/desktop/dist` unless you give one, which
  only matches the app right after the build that made it.
- `scripts/release/sign.sh <app>` signs with the Developer ID in your
  keychain, from the inside out. `--list` prints the order and signs nothing.
- `scripts/release/notarize.sh <app> <out.dmg>` notarizes and staples the app,
  makes the disk image, then notarizes and staples that. It uses the `klipolo`
  notarytool keychain profile.
- `scripts/release/check-dmg.sh <dmg>` mounts the disk image read-only.
  Gatekeeper must accept the app as Notarized Developer ID.

Before anyone downloads the disk images, the owner tags the commit they were
built from and pushes the tag to the repository at `KLIPOLO_SOURCE_URL`, so
the source that `NOTICE.txt` names is there. Pushing is a manual step:

```sh
git tag v0.1.0 <commit>
git push origin v0.1.0
```

Before a release:

- After any change to `Cargo.lock` or `pnpm-lock.yaml`, run
  `scripts/release/third-party.sh` and commit
  `crates/klipolo-app/licenses/THIRD-PARTY-NOTICES.txt`. The JavaScript part
  follows the page's imports, so importing a package the page didn't use
  before needs it too. The gate and `build-mac.sh` fail until you do.
- A merge conflict in that file: don't keep both sides. Finish the merge,
  run `scripts/release/third-party.sh` and commit what it writes.
- After changing the release scripts, run `scripts/release/release.test.sh`.
  It checks them on a small bundle made with clang, in seconds.

Tauri warns on every build that the identifier `com.klipolo.app` ends with
`.app`. It is a known cost, kept for now: the app's folders in
`~/Library/Application Support` and `~/Library/Caches` are named
`com.klipolo.app`, so Finder shows them as app packages (open them with
Show Package Contents). Changing the identifier later means moving the saved
key in the Keychain and those folders' data to the new name.

## Preview the site locally

```sh
python3 -m http.server -d site
```

Then open http://localhost:8000.

## Download links

The two download URLs (Apple Silicon and Intel) live in one place: the
`DOWNLOAD LINKS` comment in `site/index.html`. Until the first build ships they
point to the `#coming-nov-4` section. When the .dmg files are ready, swap the
two `href` values and remove that section.

## Fonts

Archivo is self-hosted from `site/assets/fonts/` under the SIL Open Font
License (see `OFL.txt` next to the font file).
