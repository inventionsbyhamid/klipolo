# Klipolo

Klipolo is a Mac video editor you talk to. Drop in raw footage, say what you
want in a chat box, and it edits. Anything that removes footage comes back as a
proposal you approve, with the removed parts shown in red on the timeline.

It runs on Apple Silicon and Intel Macs and is free while in beta.

## What Klipolo does

- **Talk to edit.** Type what you want in the chat, like "cut the ums and long
  pauses". Captions, music or a new format apply at once, with Undo. Cuts come
  back as a proposal, drawn in red on the timeline, and wait for your Apply.
- **Captions in five styles**, including karaoke, where each word lights up as
  it is said. Fix any word by hand.
- **Vertical and square versions.** Keep 16:9, 9:16 and 1:1 versions in one
  project, and export one MP4 for each.
- **Stickers and title cards drawn by Muse Image.** Klipolo cuts each sticker
  out of its background, and sets your exact title over a background with no
  lettering.
- **Edit by hand too.** Split and trim clips, add transitions, change a clip's
  speed and volume, pick Fit or Fill, and add video layers over your clips.
- **Export** MP4s drawn by the same engine as the preview, captions included,
  so the file matches what you saw.

## Meta models in Klipolo

Every model Klipolo calls is a Meta model. It calls no other company's model,
and ships none.

| Model | What it does in Klipolo | Where it's called |
| --- | --- | --- |
| Muse Spark 1.3 (`muse-spark-1.3`) | Chat: reads the project, its transcript and scene log, and edits through strict function tools. Klipolo checks every call before anything changes. | `crates/klipolo-analysis/src/providers/meta_chat.rs`, with the tools and checks in `crates/klipolo-chat` |
| Muse Spark 1.3 (`muse-spark-1.3`) | Scene log: watches a 360p copy of each clip, with its sound, and logs shots, actions, reactions, text on screen and topic changes, with times. | `crates/klipolo-analysis/src/providers/meta.rs` |
| Muse Spark 1.3 (`muse-spark-1.3`) | The MCP demo agent: edits the open project through Klipolo's MCP tools. | `scripts/mcp-demo-agent/agent.py` |
| Muse Voice Transcribe (`muse-voice-transcribe-1.0`) | Speech to text, in speaker turns with times. | `crates/klipolo-meta/src/asr.rs`, called from `crates/klipolo-analysis/src/providers/meta.rs` |
| Muse Image (`muse-image-1.0`) | Draws stickers and title card backgrounds. | `crates/klipolo-meta/src/images.rs`, called from `crates/klipolo-analysis/src/providers/meta_images.rs` |
| MMS forced aligner (torchaudio's `MMS_FA`) | Meta's open model, run on CPU on Klipolo's server. Times every word of each speaker turn, for word by word captions and cuts that land between words. | `services/relay/relay/align.py`, asked for from `crates/klipolo-meta/src/relay.rs` |

The app reaches Meta's models through one client, `crates/klipolo-meta`: with
your own key it calls Meta directly, and with an access code it goes through
the relay.

## How it fits together

- **The app** (`crates/klipolo-app`, `apps/desktop`): Tauri 2, with the work
  in Rust crates and the editor's page in React.
- **The engine** (`crates/klipolo-engine`): Rust on wgpu (Metal on the Mac)
  and FFmpeg. It draws the preview and the export on the GPU.
- **Analysis on the Mac** (`crates/klipolo-analysis`): turns each clip's sound
  into a 16 kHz mono WAV, makes a small 360p copy for Muse Spark, finds hard
  cuts from every frame, and caches the results by file content, so the same
  clip never costs a second Meta call.
- **Chat** (`crates/klipolo-chat`): the prompt, the tools, and a compiler that
  holds every cut to something the model was shown, like a word, a pause or a
  scene change.
- **The relay** (`services/relay`, at `api.klipolo.com`): registers each
  device, gives each Mac on an access code a capped budget, forwards only an
  allowlist of Meta models, and runs the MMS aligner for every device.
- **The MCP server** (`crates/klipolo-mcp`): lets other apps edit with chat's
  tools. `scripts/mcp-demo-agent` is a Muse Spark agent that drives it.

## Try it

**You need** a Mac with Apple Silicon or Intel, macOS 15 or later, and an
internet connection for the AI features.

**Download** Klipolo from klipolo.com once the first build is out (Nov 4).

**Turn on the AI features.** Open Settings (the Settings button at the top
right, or Command comma) and pick one of two ways:

- **Access code.** Choose Access code, type the code and click Use code. No
  Meta account needed. Judges: use the code given in the submission. Klipolo
  shows the credit left, and before each analysis it says what it will cost.
- **My Meta API key.** Choose My Meta API key, paste your key and click Save
  key. Klipolo checks it with Meta first, then keeps it in your Mac's
  Keychain. Meta bills your key. Get a key at dev.meta.ai.

Editing and export work without either.

**Then** click New project, drag in a short clip of someone talking, click
Analyze, and type a request in the chat.

## Let other apps edit with Klipolo (MCP)

Klipolo is also an MCP server. In Settings, under Other apps, turn on **Let
other apps edit with Klipolo**. AI agents and other MCP apps can then edit the
open project with the tools chat uses. Additions apply at once, as one undo
step. A sticker or title card background waits for your Use, and cuts always
wait for your Apply in the chat panel.

**Copy config** puts the setup for an MCP app on the clipboard. It starts
Klipolo's stdio bridge, `klipolo-app --mcp`. Klipolo listens on 127.0.0.1 only,
with a new token each run.

The demo agent is a small script where Muse Spark edits the open project
through these tools. With Klipolo open on an analyzed project and the switch
on:

```sh
cd scripts/mcp-demo-agent
python3.12 -m venv .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/python agent.py "In Klipolo, make a 30 second vertical teaser of the best moment, with karaoke captions."
```

It needs your own Meta API key in the `MODEL_API_KEY` environment variable,
and sends it only to Meta. It finds Klipolo in Applications (`--klipolo
<path>` for another copy), takes at most 8 model turns, and stops once Muse
Spark's tokens have cost $0.10 (`--max-usd` changes that). Run it again after
you click Apply, and it reads what became of the proposal.

## Repo layout

```
apps/desktop/    The app's page (React, Vite)
crates/          The Rust crates; klipolo-app is the Tauri app
scripts/         FFmpeg, Linux gate and release scripts, and the MCP demo agent
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
