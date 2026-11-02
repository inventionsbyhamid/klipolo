#!/usr/bin/env python3
"""Makes the bundled pack's sounds and pictures that come from others (scripts/library-sources.json) from their
downloads. Written into assets/library, each file's SHA-256 and its download's are recorded in the table, which
scripts/gen-library.py then checks; written anywhere else (--out), the table is left as it is, and a download that
isn't the one the table records is named.

Each sound: decoded to 48 kHz mono, cut as the table says (start and length, in seconds), high-passed at 20 Hz with
its DC removed, its quiet start and end trimmed, faded in and out (raised cosine) so it starts and ends at silence,
and set to the same loudness as the others: its loudest 400 ms at -17 LUFS (BS.1770 K-weighting), its peak no higher
than -3.5 dBFS and no lower than -9.5 dBFS, and its loudest 20 ms no louder than -9.5 dBFS. Written as 16-bit mono
WAV, under 3 s.
Each picture: the emoji's SVG drawn 896 px square in the middle of a 1024 x 1024 transparent canvas, 8-bit RGBA PNG.

Needs ffmpeg, and rsvg-convert for the pictures. The downloads are the files the table's `source` pages give
(Freesound's HQ previews as freesound/<id>.ogg, Kenney's packs unzipped under kenney/<pack>/, Noto Emoji's svg
folder at the table's commit as noto/svg/). Downloads are data: nothing in them is run.
  python3 -I scripts/library-media.py --downloads DIR [--out assets/library] [--only id,id]
"""

import argparse
import hashlib
import json
import math
import pathlib
import struct
import subprocess
import tempfile
import wave

SR = 48000
MAX_SECONDS = 2.95
TARGET_LUFS = -17.0
PEAK_MAX_DB = -3.5
PEAK_MIN_DB = -9.5
LOUDEST_20MS_DB = -9.5  # the pack's check allows -8 dBFS at most
REPO = pathlib.Path(__file__).resolve().parents[1]
TABLE = REPO / "scripts" / "library-sources.json"


def decode(path, start, length):
    args = ["ffmpeg", "-v", "error", "-i", str(path)]
    if start:
        args += ["-ss", f"{start:.4f}"]
    if length:
        args += ["-t", f"{length:.4f}"]
    args += ["-ac", "1", "-ar", str(SR), "-f", "f64le", "-"]
    raw = subprocess.run(args, check=True, capture_output=True).stdout
    return list(struct.unpack(f"<{len(raw) // 8}d", raw))


def biquad(x, b, a):
    b0, b1, b2 = b
    _, a1, a2 = a
    y, x1, x2, y1, y2 = [], 0.0, 0.0, 0.0, 0.0
    for v in x:
        out = b0 * v + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
        x2, x1, y2, y1 = x1, v, y1, out
        y.append(out)
    return y


def highpass(x, freq):
    w = 2 * math.pi * freq / SR
    alpha = math.sin(w) / (2 * math.sqrt(0.5))
    cw = math.cos(w)
    a0 = 1 + alpha
    b = ((1 + cw) / 2 / a0, -(1 + cw) / a0, (1 + cw) / 2 / a0)
    a = (1.0, -2 * cw / a0, (1 - alpha) / a0)
    return biquad(x, b, a)


def k_weighted(x):
    """BS.1770's two stages at 48 kHz."""
    x = biquad(x, (1.53512485958697, -2.69169618940638, 1.19839281085285), (1.0, -1.69065929318241, 0.73248077421585))
    return biquad(x, (1.0, -2.0, 1.0), (1.0, -1.99004745483398, 0.99007225036621))


def loudest_400ms(x):
    """The loudest 400 ms in LUFS (momentary loudness, 100 ms steps); a shorter sound counts as its whole length
    padded with silence to 400 ms, as a short sound sounds quieter."""
    k = k_weighted(x)
    window, hop = int(0.4 * SR), int(0.1 * SR)
    squares = [0.0]
    for v in k:
        squares.append(squares[-1] + v * v)
    n = len(k)
    best = max(squares[min(i + window, n)] - squares[i] for i in range(0, max(1, n - window + hop), hop))
    return -0.691 + 10 * math.log10(max(best / window, 1e-20))


def fade(x, fade_in, fade_out):
    n_in, n_out = max(1, int(fade_in * SR)), max(1, int(fade_out * SR))
    for i in range(min(n_in, len(x))):
        x[i] *= 0.5 - 0.5 * math.cos(math.pi * i / n_in)
    for i in range(min(n_out, len(x))):
        x[len(x) - 1 - i] *= 0.5 - 0.5 * math.cos(math.pi * i / n_out)
    return x


def loudest_20ms(x):
    window, hop = int(0.02 * SR), int(0.005 * SR)
    squares = [0.0]
    for v in x:
        squares.append(squares[-1] + v * v)
    n = len(x)
    best = max(squares[min(i + window, n)] - squares[i] for i in range(0, max(1, n - window + 1), hop))
    return 10 * math.log10(max(best / min(window, n), 1e-20))


def trim(x, lead_db, tail_db, keep_lead=0.002, keep_tail=0.006):
    peak = max(abs(v) for v in x)
    lo, hi = peak * 10 ** (lead_db / 20), peak * 10 ** (tail_db / 20)
    first = next(i for i, v in enumerate(x) if abs(v) >= lo)
    last = max(i for i, v in enumerate(x) if abs(v) >= hi)
    return x[max(0, first - int(keep_lead * SR)):min(len(x), last + int(keep_tail * SR))]


def sound(item, downloads):
    x = decode(downloads / item["from"], item.get("start", 0.0), item.get("length"))
    x = highpass(x, 20.0)
    mean = sum(x) / len(x)
    x = [v - mean for v in x]
    cut_start, cut_end = item.get("start", 0.0) > 0, "length" in item
    # The quiet start and end of a whole sound go; an excerpt's cut ends are faded as they are.
    x = trim(x, -50.0 if not cut_start else -90.0, -60.0 if not cut_end else -90.0)
    if len(x) > MAX_SECONDS * SR:
        x = x[:int(MAX_SECONDS * SR)]
        cut_end = True
    x = fade(x, item.get("fade_in", 0.005 if cut_start else 0.002), item.get("fade_out", 0.4 if cut_end else 0.02))
    # After the fades, what is under a thousandth of the peak at either end goes too (the pack's check allows
    # 0.1 s before and 0.05 s after), and the ends are faded to silence again.
    x = trim(x, -60.0, -60.0, 0.001, 0.004)
    x = fade(x, 0.001, 0.003)
    # What DC is left (a very short click is lopsided) goes in the shape of the fades, so the ends stay at silence.
    shape = fade([1.0] * len(x), 0.001, 0.003)
    offset = sum(x) / sum(shape)
    x = [v - offset * w for v, w in zip(x, shape)]
    peak = max(abs(v) for v in x)
    gain = 10 ** ((TARGET_LUFS - loudest_400ms(x)) / 20)
    gain = min(gain, 10 ** (PEAK_MAX_DB / 20) / peak, 10 ** ((LOUDEST_20MS_DB - loudest_20ms(x)) / 20))
    gain = max(gain, 10 ** (PEAK_MIN_DB / 20) / peak)
    pcm = [max(-32767, min(32767, round(v * gain * 32767))) for v in x]
    pcm[0] = pcm[-1] = 0
    out = tempfile.SpooledTemporaryFile()
    with wave.open(out, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(struct.pack(f"<{len(pcm)}h", *pcm))
    out.seek(0)
    return out.read(), f"{len(pcm) / SR:.2f} s, gain {20 * math.log10(gain):+.1f} dB"


def sticker(item, downloads):
    with tempfile.TemporaryDirectory() as tmp:
        drawn = pathlib.Path(tmp) / "drawn.png"
        out = pathlib.Path(tmp) / "out.png"
        subprocess.run(["rsvg-convert", "-w", "896", "-h", "896", "-a", "-o", str(drawn),
                        str(downloads / item["from"])], check=True)
        subprocess.run(["ffmpeg", "-v", "error", "-i", str(drawn), "-vf",
                        "format=rgba,pad=1024:1024:(ow-iw)/2:(oh-ih)/2:color=0x00000000", "-pix_fmt", "rgba",
                        "-pred", "mixed", "-y", str(out)], check=True)
        return out.read_bytes(), "1024 x 1024"


def main():
    parser = argparse.ArgumentParser(description="Make the pack's items from others from their downloads.")
    parser.add_argument("--downloads", type=pathlib.Path, required=True)
    parser.add_argument("--out", type=pathlib.Path, default=REPO / "assets" / "library")
    parser.add_argument("--only", help="comma-separated ids")
    args = parser.parse_args()
    table = json.loads(TABLE.read_text())
    only = set(args.only.split(",")) if args.only else None
    record = args.out.resolve() == (REPO / "assets" / "library").resolve()
    for item in table["items"]:
        if only and item["id"] not in only:
            continue
        folder, ext, make = (("sounds", "wav", sound) if item["category"] == "sound" else ("stickers", "png", sticker))
        download = hashlib.sha256((args.downloads / item["from"]).read_bytes()).hexdigest()
        data, said = make(item, args.downloads)
        target = args.out / folder / f"{item['id']}.{ext}"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        if record:
            item["download_sha256"] = download
            item["sha256"] = hashlib.sha256(data).hexdigest()
        elif item.get("download_sha256") not in (None, download):
            print(f"{item['id']}: {item['from']} isn't the download the table records")
        print(f"{item['id']:<16} {said:<26} {len(data):>8} bytes  {item['source']}")
    if record:
        TABLE.write_text(json.dumps(table, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
