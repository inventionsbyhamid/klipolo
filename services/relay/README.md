# Klipolo relay

The relay lets people with an access code use Meta's models in Klipolo without their own key, under hard spending caps. It also gives every device word timing for its transcripts. It runs at `https://api.klipolo.com`.

## What it serves

| Route | What it does |
| --- | --- |
| `GET /healthz` | `{"ok": true}` |
| `POST /v1/devices` | Registers a device, with or without an access code. Returns a bearer token. A code at its device limit takes no new devices (402 `budget`, "Klipolo's shared AI credit is used up for now."), but its own devices may register again. |
| `GET /v1/devices/me` | Whether relay AI is on, the budget and the spend. The credit left is the budget less the spend: when the device's code has a cap with less left than the device, the budget is cut to match. |
| `POST /meta/v1/responses`, `POST /meta/v1/asr/transcribe`, `POST /meta/v1/images/generations`, `POST /meta/v1/files`, `GET` and `DELETE /meta/v1/files/{id}` | Forwarded to Meta for devices with an active code, and charged against the caps. |
| `POST /meta/v1/sam/videos` | A video for SAM, sent as its raw bytes (at most 64 MB, one a device at a time): the relay decodes it with ffmpeg and writes its own copy (one video track, 2 frames a second, its long side at most 640, no sound, at most 10 minutes), uploads that to Meta and answers with Meta's file. SAM takes only videos made this way, and the relay reserves their frames. ffmpeg reads only MOV/MP4 from a file, on two threads, pictures up to 4K, under a 1.5 GB memory limit at a lower priority; two copies are made at once, a third waits at most 10 s (then 429), and each has 60 s. |
| `POST /v1/align` | Word timing for transcript turns: multipart `audio` (16 kHz mono 16-bit WAV, at most 10 minutes) and `turns` (JSON). Open to every registered device, 120 minutes a day. Turns in any script are timed: words in other scripts than Latin are romanized with uroman first (see Third-party software). |

Errors are always `{"error": "<code>", "message": "<sentence>"}`, except Traefik's rate limit (see Limits to know). Without a Meta key the proxied routes answer 503 `not_configured`.

## The client contract

What Klipolo, or any other client, must do to use the relay well:

- **In flight per device:** at most 4 proxied calls and 2 alignments. More get 429 `busy`.
- **429 is retryable, 402 is final.** On 429, wait for `Retry-After` and try again. A 402 `budget` stays until the device or the relay has credit again; the message says which, and the app should offer the user's own Meta key.
  - 402 "This device has used its Klipolo credit." The device's spend reached its cap.
  - 402 "This device does not have enough Klipolo credit left for this request." The device has some credit, but less than this call could cost. A smaller call may still fit.
  - 402 "Klipolo's shared AI credit is used up for now." The relay's overall cap is reached, or the access code's cap: what all the code's devices spent together.
  - 429 "Other AI requests from this device are still running." Its other calls in flight hold the room until they finish. `Retry-After: 5`.
  - 429 "Klipolo AI is busy." Calls in flight from other devices hold the shared room. `Retry-After: 5`.
  - Other 429 `busy`: the device already has 4 proxied calls (or 2 alignments) in flight, or the relay has 32 proxied calls (or 8 alignments). `Retry-After` is 1 s for proxied calls and 30 s for alignment.
  - 429 `align_quota`: the device has used its alignment minutes for the day. `Retry-After` points at midnight UTC.
- **Video calls one at a time per device.** Each reserves about $0.79 until it is charged, so three at once don't fit in a $2 cap.
- **Alignment read timeout of at least 15 minutes.** Two 10-minute alignments at once take about 10.5 minutes each, and a request can wait in the queue first.
- **Uploads must finish within 60 s** (Traefik's body read). Keep video proxies small; see Limits to know.
- **`max_output_tokens` at most 16384** (8192 when missing). More is 400.
- **No `include` or `truncation`**, or any other field the relay doesn't list: 400. No `previous_response_id` or `conversation` either.
- **Don't replay reasoning items.** Any reasoning item in `input` makes the call count as media, which reserves $0.75 more.
- **Muse Image (`images/generations`): `muse-image-1.0` only, `n` at most 2.** More is 400. Its tools (image search, web search, shell) are always turned off. Each image returned costs $0.01, up to the `n` asked for, and a call reserves $0.02 until it is charged. No streaming, edits or `moderation`.
- **A prompt Meta won't serve** comes back 400 with the code `content_policy_violation` and Meta's reason. Every other Meta error is `meta_error`.

## Settings

All settings come from the environment.

| Variable | Default | Meaning |
| --- | --- | --- |
| `META_API_KEY` | empty | The server's own Meta key. Never logged. |
| `DEVICE_CAP_USD` | `2.00` | Most a device can spend, all time. |
| `GLOBAL_CAP_USD` | `40.00` | Most the whole relay can spend, all time. |
| `ALLOWED_MODELS` | `muse-spark-1.3,muse-spark-1.2,muse-voice-transcribe-1.0,muse-image-1.0,sam-3.1` | Models the proxy forwards. SAM's video calls must stream, on a video the relay made (`POST /meta/v1/sam/videos`): its frames are reserved (at least 50 a video) and charged as Meta bills them. If Meta bills more frames than were reserved (past 2% and 2 frames of slack), SAM goes off for that access code until `enable-sam`. Muse Wonder and the contributor tiers are refused even when listed. |
| `SAM_DAILY_CAP_USD` | `5.00` | Most SAM may cost in a day (UTC), across every device, on top of the other caps. |
| `SAM_WORK_DIR` | `sam-work` beside the database | Where SAM's uploads and copies are made (not `/tmp`, which word timing uses). Below 192 MB free, SAM uploads get 503. |
| `ALIGN_MINUTES_PER_DEVICE_PER_DAY` | `120` | Word timing per device per UTC day. |
| `ALIGN_CONCURRENCY` | `2` | Alignments running at once. More than 2 can run out of memory in 6 GB. |
| `ALIGN_THREADS` | `3` | Threads per alignment. |
| `ALIGN_WARMUP` | off (on in the image) | Load the alignment model at startup. |
| `RELAY_DB_PATH` | `./data/relay.db` (`/data/relay.db` in the image) | The SQLite database. |

Run one worker only: the limits on calls in flight and the alignment queue live in the process.

## Run locally

```sh
cd services/relay
uv venv && uv pip install -e '.[dev]'
.venv/bin/uvicorn --factory relay.app:create_app --port 8080
```

Alignment needs torch 2.8 and the MMS model, which live in the container image. Without them `/v1/align` answers 503 `align_unavailable`.

The romanizer (uroman) is a plain Python package installed with the rest. It takes about 10 s to load its tables, at startup with `ALIGN_WARMUP` on, otherwise on the first turn with words in another script.

## Test

The fast suite needs no torch and never calls Meta:

```sh
.venv/bin/pytest -q
```

The full suite, with the slow alignment tests, runs in the image. Run it on the server, in the build folder (see Deploy):

```sh
cd <build folder>
docker run --rm --cpus 8 --memory 6g --user root -v "$PWD:/src:ro" -w /src klipolo-relay:<tag> sh -c '
  pip install -q --root-user-action=ignore "pytest>=8" "httpx2>=2.16" "respx>=0.22" &&
  python -m pytest -p no:cacheprovider -m "slow or not slow" -q tests'
```

## Deploy

The relay is the Docker Compose service `klipolo-relay` in the Coolify project `klipolo`, built from `compose.yml`. The image is built on the server and never pulled; `RELAY_TAG` in Coolify picks the tag. Traefik routes `api.klipolo.com` to it with a Let's Encrypt certificate. It has no host ports, 8 CPUs, 6 GB, a data volume and a 200 MB in-memory `/tmp`.

To run your own relay, put your own domain in place of `api.klipolo.com`, in `compose.yml`'s Traefik labels and in step 6's health check.

**Never set a domain for this service in Coolify.** The routing comes from the Traefik labels in `compose.yml`. A Coolify domain adds a second router with Coolify's gzip middleware, which can hold back small streamed events, and it would skip the rate limits.

When `compose.yml` changes, paste it into the service's compose in Coolify (or send it through the API) before the restart.

1. Copy the committed source to the server:

   ```sh
   git archive HEAD services/relay | ssh <you>@<server> 'rm -rf <build folder> && mkdir -p <build folder> && tar -x --strip-components=2 -C <build folder>'
   ```

2. Build with capped CPU and memory. The classic builder applies the caps; BuildKit ignores them.

   ```sh
   TAG=$(git rev-parse --short=7 HEAD)
   ssh <you>@<server> "cd <build folder> && DOCKER_BUILDKIT=0 nice -n 10 docker build --cpu-quota 800000 --cpu-period 100000 --memory 6g -t klipolo-relay:$TAG ."
   ```

3. Run the full suite in the new image (see Test).
4. Pin the image, so the server's image cleanup keeps it even while the service is stopped:

   ```sh
   ssh <you>@<server> "docker create --name keep-klipolo-relay-$TAG --label purpose=pin-image --network none klipolo-relay:$TAG true"
   ```

   Keep the previous tag's pin until the new one has run for a day, then remove it with `docker rm keep-klipolo-relay-<old tag>` and `docker rmi klipolo-relay:<old tag>`.
5. In Coolify, set `RELAY_TAG` to the new tag under the service's Environment Variables, then restart the service (see below).
6. Check: `curl -s https://api.klipolo.com/healthz` gives `{"ok":true}`.

To roll back, set `RELAY_TAG` to the previous pinned tag and restart.

**Restart only when the relay is idle.** On SIGTERM the relay finishes the calls in flight, including streams it drains after a client left and alignments. `compose.yml` gives it 330 s. Only a redeploy through Coolify's API keeps that: `PATCH /api/v1/services/<uuid>` with `{"instant_deploy": true}`, which recreates the container without stopping it first (the API's `start` refuses a running service). The Restart and Stop buttons allow 30 s. Neither is enough for two 10-minute alignments at once, which take about 10.5 minutes. A call cut off mid-stream is billed by Meta but never recorded. Before a restart, check that nothing is running:

```sh
C=$(docker ps --filter name=^klipolo-relay- --format '{{.Names}}')
docker stats --no-stream $C     # CPU near 0%: no alignment running
docker logs --since 5m $C       # no recent requests or charges
docker exec $C python -m relay.admin spend
```

Temp audio lives in the in-memory `/tmp`, so a crash or restart never leaves any behind.

Coolify writes the variables into the compose file when it starts the service. A changed value takes effect only after a restart.

## Add the server's Meta key

The relay deploys with an empty key, and the proxied routes answer 503 "Relay not configured." until the key is set. Word timing and registration work without it.

1. Create a separate key for the server on dev.meta.ai. Don't reuse the development key.
2. In Coolify, open project `klipolo`, environment `production`, service `klipolo-relay`, then Environment Variables.
3. Set `META_API_KEY` to the key and save.
4. Restart the service, while it is idle.
5. In Klipolo, with an access code and no key of your own, run one analysis. It should answer instead of saying the relay is not configured.

## Admin commands

The admin CLI works on the live database, and changes take effect on the next request. Run it in the container:

```sh
C=$(docker ps --filter name=^klipolo-relay- --format '{{.Names}}')
docker exec $C python -m relay.admin add-code <code> --note "who it is for"
docker exec $C python -m relay.admin deactivate-code <code>
docker exec $C python -m relay.admin enable-sam <code>
docker exec $C python -m relay.admin set-code-cap <code> --usd 30 --devices 25
docker exec $C python -m relay.admin list-codes
docker exec $C python -m relay.admin spend
```

- `add-code` also reactivates a code that was turned off.
- `deactivate-code` stops relay AI at once for every device that registered with the code.
- `enable-sam` turns video tracking back on for a code the relay turned it off for (Meta billed more frames than it reserved: look at the log first).
- `set-code-cap` limits one code, on top of the per-device and overall caps: `--usd` is the most all its devices may spend together, all time, and `--devices` how many devices may register with it, counting every device that ever did. Give either or both; `none` removes a limit. A new code has neither, so nothing changes until you set them. Give a code that many people share (judges, a class) both: without them, anyone with the code can register new devices, $2 each, until the overall cap is gone for everyone.
- `list-codes` shows each code's devices (with its limit, as `3/25`), what was charged under it and its cap.
- `spend` shows the total, the spend per device and the last 20 charges.

Codes are case-sensitive. Make them hard to guess: at least 12 random lowercase letters and digits.

## Logs

`docker logs` shows uvicorn's access lines and the relay's own INFO lines: charges, alignment timings, and the alignment model loading. They never hold request or response headers or bodies, or the key. Device ids appear as their first 8 characters.

## Data

The SQLite database is `/data/relay.db` in the service's volume, which ends in `_klipolo-relay-data`. It holds codes (with their limits), devices (token hashes only), which devices registered with which code, and one ledger row per charged call, with the code it was charged under. Nothing from requests or audio is kept. To copy it while the relay runs:

```sh
docker exec $C python -c "import sqlite3; sqlite3.connect('/data/relay.db').backup(sqlite3.connect('/data/backup.db'))"
docker cp $C:/data/backup.db relay-backup.db && docker exec $C rm /data/backup.db
```

The relay moves the database to its current schema when it opens it, in one transaction. Schema 3 adds the per-code limits: two columns on codes (no limits), the code on each ledger row (filled in for earlier rows from their device's code) and the table of devices registered with each code (filled in from the devices on each code). It only adds, so an earlier relay still runs on it (a rollback, or the old container during a deploy). The charges and registrations an earlier relay writes lack their code; the next time this relay opens the database it fills them in the same way, and only takes the write lock when there is something to fill in.

## Limits to know

- Traefik reads a request body for at most 60 s. The largest alignment upload (19 MB) needs about 3 Mbps, and the largest transcribe upload (32 MB) about 4.5 Mbps. File uploads to `/meta/v1/files` have the same 60 s, about 75 MB at 10 Mbps, so keep video proxies small.
- Traefik limits each client IP to 10 requests a second, with bursts of 30, and `POST /v1/devices` to 10 a minute, with bursts of 5. Over the limit it answers 429 with a plain-text body, not the JSON shape.
- With 3 threads, an alignment takes roughly as long as its audio: up to about 10 minutes for 10 minutes of audio, when two run at once. Clients need long read timeouts, or should send a few minutes at a time.

## Update the pinned packages

The image installs exact versions from two lock files, with `--no-deps`:

- `requirements-torch.lock`: torch, torchaudio, numpy and what they need, from the PyTorch CPU index.
- `requirements.lock`: everything else, from PyPI.

The image only installs these pins, so `pip freeze` inside it gives the same list back. To pick up new versions, resolve the unpinned requirements in a clean container instead. Run this in `services/relay`:

```sh
docker run --rm --cpus 4 --memory 4g -v "$PWD:/src:ro" python:3.12-slim-bookworm sh -c '
  pip install -q --root-user-action=ignore --index-url https://download.pytorch.org/whl/cpu \
    torch==2.8.0 torchaudio==2.8.0 numpy &&
  pip freeze --exclude pip > /tmp/torch.lock &&
  pip install -q --root-user-action=ignore /src &&
  pip freeze --exclude pip | grep -v "^klipolo-relay" | grep -vxF -f /tmp/torch.lock > /tmp/rest.lock;
  echo "== requirements-torch.lock"; cat /tmp/torch.lock; echo "== requirements.lock"; cat /tmp/rest.lock'
```

Copy each list under the comment header of its lock file, rebuild the image, and run the full test suite in it (see Test). torch and torchaudio stay at 2.8: it is the last release with `forced_align`.

## Move the base image

The Dockerfile pins `python:3.12-slim-bookworm` by its multi-platform index digest. To move to a newer build of the same tag:

```sh
docker buildx imagetools inspect python:3.12-slim-bookworm | grep -m1 Digest
```

Put that digest after `@sha256:` in the `FROM` line, rebuild, and run the full test suite.

## Third-party software

The relay romanizes words in other scripts than Latin with uroman 1.3.1.1 (https://github.com/isi-nlp/uroman), under its MIT-style licence, which asks for this note: This project uses the universal romanizer software 'uroman' written by Ulf Hermjakob, USC Information Sciences Institute (2015-2020). Ulf Hermjakob, Jonathan May, and Kevin Knight. 2018. Out-of-the-box universal romanization tool uroman. In Proceedings of the 56th Annual Meeting of Association for Computational Linguistics, Demo Track.
