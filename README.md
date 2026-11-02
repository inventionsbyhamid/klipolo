# Klipolo

Klipolo is a Mac video editor you edit by chatting with Meta's models.

Record your screen in Klipolo, or drop in a clip. Then type one request:

> Turn this into a 60 second vertical short with nice animations, sound effects
> and text animations.

Muse Spark 1.3 finds the best moments and plans the whole short: the cut, a
vertical crop, captions that pop, a hook title, stickers with sounds, and music
that dips under your voice. The cut waits in red on the timeline until you click
Apply. Then export it or share it.

Klipolo runs on Apple Silicon and Intel Macs with macOS 15 or later. It is free
while in beta.

## Try it

**1. Download** the disk image for your Mac:

- Apple Silicon: [Klipolo for Apple Silicon](https://github.com/inventionsbyhamid/klipolo/releases/download/v0.1.0/Klipolo-0.1.0-aarch64.dmg)
- Intel: [Klipolo for Intel](https://github.com/inventionsbyhamid/klipolo/releases/download/v0.1.0/Klipolo-0.1.0-x86_64.dmg)

Open it and drag Klipolo to Applications.

**2. Turn on the AI features.** Open Settings (the Settings button at the top
right, or Command comma) and pick one of two ways:

- **Access code.** Choose Access code, type the code and click Use code. No
  Meta account needed. Judges: use the code given in the submission. Each Mac
  gets $2 of credit, and Klipolo shows what is left.
- **My Meta API key.** Choose My Meta API key, paste your key and click Save
  key. Klipolo checks it with Meta first, then keeps it in your Mac's Keychain.
  Meta bills your key. Get a key at dev.meta.ai.

Klipolo shows the price before each analysis. The clip card shows it before
following a speaker. Recording, editing and export work without a code or a
key. The AI features need the internet.

**3. Make a short.**

1. Click **Record your screen**. Pick your whole screen or one app, then your
   mic and camera, and record. Or pick **Record yourself** for just your camera
   and mic, with no screen. Stop with Option Command R. Or click
   **New project** and drag in a video of people talking.
2. Click **Analyze** at the top of the preview.
3. Type in the chat: "Turn this into a 60 second vertical short with nice
   animations, sound effects and text animations."
4. Click **Apply** on the cut, then **Export**. When it's done, click
   **Share…**.

## What Klipolo does

- **Record in the app.** Your whole screen or one app, with your mic and
  camera. A take lands in your library with auto zoom on your clicks. Its
  vertical version follows your cursor. Or record yourself: your camera and
  mic, with no screen.
- **One message, a whole short.** Muse Spark finds the best moments first, then
  plans the rest in one go. It adapts the plan to what you asked for.
- **You approve every cut.** Anything that removes footage waits in red until
  you click Apply. Everything else applies at once, as one step to undo.
- **Cuts it can't make up.** Every cut must start and end near something the
  model was shown, like a word, a pause or a scene change, or a time you typed.
  Klipolo snaps it there, or sends it back to the model with the reason.
- **Keeps the speaker in frame.** In footage of people talking, SAM 3.1
  tracks each person. Muse Spark matches each voice to a person, so a vertical
  crop keeps whoever is talking. Or ask: "keep the guest in frame".
- **Captions, word by word.** Seven styles, two of them made for shorts. Fix
  any word by hand. Save them as an SRT or WebVTT file too.
- **Motion from plain words.** "Make the title glide in from the left with a
  soft overshoot." Muse Spark sets keyframes with springs, animates words
  letter by letter, and adds effects and volume changes over time.
- **Stickers and title cards by Muse Image.** Klipolo cuts each sticker out of
  its white background. Your title stays your exact words, over a background
  with no lettering.
- **A full editor by hand.** Split and trim, transitions, speed, keyframes for
  motion and volume, an effects stack, blurred video or color backgrounds for
  shorts, roll, slip and slide, freeze frame, reverse, video layers, free fonts
  and safe areas.
- **Export and share.** One MP4 for each format (16:9, 9:16 and 1:1), up to
  Master quality. The same engine draws the preview and the export, so the
  file matches what you saw. Share… opens the Mac's share menu.
- **Other apps can edit too.** Klipolo is an MCP server with chat's own tools.

## Meta models in Klipolo

Every model Klipolo calls is a Meta model. It calls no other company's model,
and ships none.

| Model | What it does in Klipolo | Where it's called |
| --- | --- | --- |
| Muse Spark 1.3 (`muse-spark-1.3`) | Chat: reads the project, its transcript and scene log, and edits through strict function tools, up to 10 model calls a message. Klipolo checks every call before anything changes. | `crates/klipolo-analysis/src/providers/meta_chat.rs`, with the tools and checks in `crates/klipolo-chat` |
| Muse Spark 1.3 | Scene log: watches a 360p copy of each imported clip, with its sound, and logs shots, actions, reactions, text on screen and topic changes, with times. | `crates/klipolo-analysis/src/providers/meta.rs` |
| Muse Spark 1.3 | Who is speaking: watches a few seconds of a clip, with its sound, and the people SAM 3.1 found, and says whose voice it hears. | `crates/klipolo-analysis/src/providers/meta_voices.rs` |
| Muse Spark 1.3 | The MCP demo agent: edits the open project through Klipolo's MCP tools. | `scripts/mcp-demo-agent/agent.py` |
| Muse Voice Transcribe (`muse-voice-transcribe-1.0`) | Speech to text, in speaker turns, each with a voice label. | `crates/klipolo-meta/src/asr.rs`, called from `crates/klipolo-analysis/src/providers/meta.rs` |
| SAM 3.1 (`sam-3.1`) | Tracks every person in the footage that plays, on a small copy at 2 frames a second. Its boxes become the path of the vertical or square crop. | `crates/klipolo-sam`, called from `crates/klipolo-analysis/src/providers/meta_sam.rs` |
| Muse Image (`muse-image-1.0`) | Draws stickers and title card backgrounds. | `crates/klipolo-meta/src/images.rs`, called from `crates/klipolo-analysis/src/providers/meta_images.rs` |
| MMS forced aligner (`MMS_FA`) | Meta's open model, run on CPU on Klipolo's relay. Times every word of each speaker turn, for word by word captions and cuts that land between words. Words it scores near zero are dropped. | `services/relay/relay/align.py`, asked for from `crates/klipolo-meta/src/relay.rs` |

The app reaches Meta's models through one client, `crates/klipolo-meta`. With
your own key it calls Meta directly. With an access code it goes through the
relay.

## How it fits together

- **The app** (`crates/klipolo-app`, `apps/desktop`): a Tauri 2 app. Rust
  crates do the work, and a React page draws the editor.
- **The engine** (`crates/klipolo-engine`): Rust on the Mac's GPU, with
  FFmpeg. It draws both the preview and the export.
- **The recorder** (`crates/klipolo-capture`): each source goes to its own
  file, written by a worker process that survives an editor crash.
- **Analysis on the Mac** (`crates/klipolo-analysis`): turns each clip's sound
  into a 16 kHz mono WAV, makes a small 360p copy for Muse Spark, finds hard
  cuts from every frame, and caches the results by file content. The same clip
  never costs a second Meta call.
- **Chat** (`crates/klipolo-chat`): the prompt, the tools, and a compiler that
  holds every cut to something the model was shown.
- **The relay** (`services/relay`, at `api.klipolo.com`): registers each
  device, gives each Mac on an access code a capped budget, forwards only an
  allowlist of Meta models, and runs the MMS aligner for every device.
- **The MCP server** (`crates/klipolo-mcp`): lets other apps edit with chat's
  tools.

## Let other apps edit with Klipolo (MCP)

Klipolo is also an MCP server. In Settings, under Other apps, turn on **Let
other apps edit with Klipolo**. AI agents and other MCP apps can then edit the
open project with the tools chat uses, checked by the same compiler. Additions
apply at once, as one undo step. A sticker or title card background waits for
your Use, and cuts always wait for your Apply in the chat panel.

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
apps/desktop/      The app's page
crates/            The Rust crates; klipolo-app is the app
services/relay/    Klipolo's relay
scripts/           FFmpeg, test and release scripts, and the MCP demo agent
assets/            Fonts and the pack of music, sounds, stickers and backgrounds
site/              klipolo.com (static, hand written, no build step)
```

## Build from source

You need macOS 15, the Xcode Command Line Tools, rustup (it installs the Rust
version in `rust-toolchain.toml`), Node 22 and pnpm 11.

1. Build Klipolo's own LGPL FFmpeg 9.0.2, or point this checkout at a build
   you already have:

   ```sh
   scripts/build-ffmpeg.sh                   # this Mac's arch
   scripts/build-ffmpeg.sh --arch arm64      # or the other one
   scripts/link-ffmpeg.sh <build dir>        # or link a shared build
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
   pnpm tauri dev
   ```

**Tests.** `scripts/ci-linux.sh` is the gate. It runs rustfmt, clippy, the
Rust tests, the GPU tests on a software renderer, the page's typecheck and
tests, and checks that the bindings and the third party notices are current.
Run it in the image from `ci/linux.Dockerfile`:

```sh
docker build -f ci/linux.Dockerfile -t klipolo-ci ci
docker run --rm -v "$PWD":/src -w /src klipolo-ci scripts/ci-linux.sh
```

The image runs as uid 1000, so the checkout must belong to that user.

**Disk images.** `scripts/release/build-mac.sh <x86_64|aarch64> <version>`
builds the app, checks the bundle (`check-bundle.sh`), signs it (`sign.sh`),
notarizes the app and the disk image (`notarize.sh`) and checks that
Gatekeeper accepts it (`check-dmg.sh`). It writes
`dist/Klipolo-<version>-<arch>.dmg` and needs `KLIPOLO_SOURCE_URL`, the public
source address that the app's `NOTICE.txt` gives. After any change to
`Cargo.lock` or `pnpm-lock.yaml`, run `scripts/release/third-party.sh` and
commit the notices it writes.

**The site** is static. Preview it with `python3 -m http.server -d site`, then
open http://localhost:8000. Its two download links are in the
`DOWNLOAD LINKS` comment in `site/index.html`.

## Licence

- Klipolo is free software under the GNU Affero General Public License,
  version 3 (`LICENSE`).
- Parts of the video engine and the recorder are derived from other open
  source code, used under AGPL 3.0 and MIT. `crates/klipolo-engine/NOTICE` and
  `crates/klipolo-capture/NOTICE` name each file and where it came from.
- FFmpeg is used under the LGPL, version 2.1 or later, and is dynamically
  linked.
- The pack's music, backgrounds and own stickers are made from code by
  `scripts/gen-library.py` and dedicated to the public domain under CC0 1.0.
  Its sound effects come from Freesound and Kenney under CC0 1.0, and its emoji
  stickers are Google's Noto Emoji under the Apache License 2.0.
  `assets/library/LICENSE.txt` credits each file, and
  `scripts/library-sources.json` lists where each one came from.
- Each font in `assets/fonts` comes with its own licence file. Archivo, the
  interface and site font, is under the SIL Open Font License
  (`site/assets/fonts/OFL.txt`).
- The Rust crates and JavaScript packages the app uses are listed with their
  licences in `crates/klipolo-app/licenses/THIRD-PARTY-NOTICES.txt`.
- The relay (`services/relay`) romanizes words in other scripts for word
  timing with uroman (https://github.com/isi-nlp/uroman), under its MIT-style
  licence: "This project uses the universal romanizer software 'uroman'
  written by Ulf Hermjakob, USC Information Sciences Institute (2015-2020)".
