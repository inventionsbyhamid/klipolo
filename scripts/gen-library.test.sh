#!/usr/bin/env bash
# The bundled pack's checks (Python 3 stdlib only): scripts/gen-library.py run twice into temporary folders writes the
# same bytes both times, and the same bytes as assets/library, whose manifest lists each file's SHA-256; every sound is
# 48 kHz 16-bit mono and under 3 s; every music bed is 16 to 30 s and loops at a zero crossing; every sticker is a
# 1024 x 1024 RGBA PNG with transparent corners; the pack is the one the plan lists, CC0-1.0, and under 3 MB.
#   scripts/gen-library.test.sh
set -euo pipefail
cd "$(dirname "$0")/.."

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
python3 scripts/gen-library.py --out "$tmp/a" >/dev/null
python3 scripts/gen-library.py --out "$tmp/b" >/dev/null

python3 -I - "$tmp/a" "$tmp/b" assets/library <<'PY'
import hashlib, json, pathlib, struct, sys, wave, zlib

first, second, committed = (pathlib.Path(p) for p in sys.argv[1:4])
failed = []


def check(name):
    def run(test):
        try:
            test()
            print(f"ok    {name}")
        except AssertionError as e:
            print(f"FAIL  {name}: {e}")
            failed.append(name)
    return run


def files(root):
    return {str(p.relative_to(root)): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file()}


manifest = json.loads((committed / "manifest.json").read_text())
items = manifest["items"]


def of(category):
    return [i for i in items if i["category"] == category]


@check("library_is_deterministic_and_matches_manifest")
def _():
    a, b, c = files(first), files(second), files(committed)
    assert a == b, f"two runs differ: {sorted(k for k in a if a.get(k) != b.get(k))}"
    differ = sorted(set(a) ^ set(c) | {k for k in a if a[k] != c.get(k)})
    assert a == c, f"assets/library isn't what the script writes: {differ}"
    listed = {i["file"] for i in items}
    assert listed | {"manifest.json", "LICENSE.txt"} == set(c), f"unlisted or missing: {sorted(listed ^ set(c))}"
    for i in items:
        got = hashlib.sha256(c[i["file"]]).hexdigest()
        assert got == i["sha256"], f"{i['id']}: sha256 {got}, manifest {i['sha256']}"


@check("manifest_lists_the_pack")
def _():
    want = {
        "sound": ["whoosh", "swoosh", "pop", "click", "ding", "chime", "rise", "sparkle"],
        "music": ["soft-ambient", "upbeat-loop"],
        "sticker": ["heart", "star", "arrow", "circle", "check", "bubble"],
    }
    assert {k: [i["id"] for i in of(k)] for k in want} == want, [i["id"] for i in items]
    assert len(items) == 16 and manifest["license"] == "CC0-1.0" and manifest["creator"] == "Klipolo"
    for i in items:
        assert i["license"] == "CC0-1.0", i["id"]
        assert i["name"] and not any(d in i["name"] for d in "–—"), i["id"]
        assert i["file"].startswith(f"{i['category']}s/" if i["category"] != "music" else "music/"), i["file"]
        assert ("duration_ms" in i) == (i["category"] != "sticker"), i["id"]
        assert ("width" in i and "height" in i) == (i["category"] == "sticker"), i["id"]
    size = sum(len(v) for v in files(committed).values())
    assert size < 3 * 1024 * 1024, f"the pack is {size} bytes"


def pcm(path):
    with wave.open(str(path), "rb") as w:
        params = w.getparams()
        frames = w.readframes(params.nframes)
    return params, struct.unpack(f"<{params.nframes}h", frames)


@check("every_sound_is_48k_and_under_3s")
def _():
    for i in of("sound"):
        p, s = pcm(committed / i["file"])
        assert (p.nchannels, p.sampwidth, p.framerate) == (1, 2, 48000), f"{i['id']}: {p}"
        assert p.nframes < 3 * 48000, f"{i['id']}: {p.nframes / 48000:.2f} s"
        assert i["duration_ms"] == round(p.nframes * 1000 / 48000), i["id"]
        peak = max(abs(v) for v in s)
        assert 3000 < peak < 32767, f"{i['id']}: peak {peak}"
        assert abs(s[0]) <= 64 and abs(s[-1]) <= 64, f"{i['id']}: starts or ends with a click"


@check("music_loops_at_a_zero_crossing")
def _():
    for i in of("music"):
        p, s = pcm(committed / i["file"])
        assert (p.nchannels, p.sampwidth) == (1, 2), f"{i['id']}: {p}"
        seconds = p.nframes / p.framerate
        assert 16 <= seconds <= 30, f"{i['id']}: {seconds:.2f} s"
        assert i["duration_ms"] == round(p.nframes * 1000 / p.framerate), i["id"]
        step = max(abs(b - a) for a, b in zip(s, s[1:]))
        assert abs(s[0]) <= 64 and s[-1] * s[0] <= 0, f"{i['id']}: the seam isn't at a zero crossing ({s[-1]}, {s[0]})"
        assert abs(s[0] - s[-1]) <= step, f"{i['id']}: the seam jumps {abs(s[0] - s[-1])}, the largest step is {step}"
        peak = max(abs(v) for v in s)
        assert 8000 < peak < 32767, f"{i['id']}: peak {peak}"


def rgba_rows(path):
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos, idat, header = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack("!I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack("!2I5B", body)
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    w, h, depth, colour, _, _, interlace = header
    assert (depth, colour, interlace) == (8, 6, 0), f"8-bit RGBA, not interlaced: {header}"
    raw, stride, rows, prev = zlib.decompress(idat), w * 4, [], bytearray(w * 4)
    for y in range(h):
        kind, line = raw[y * (stride + 1)], bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for x in range(stride):
            a, b, c = line[x - 4] if x >= 4 else 0, prev[x], prev[x - 4] if x >= 4 else 0
            if kind == 1:
                line[x] = (line[x] + a) & 255
            elif kind == 2:
                line[x] = (line[x] + b) & 255
            elif kind == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif kind == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line)
        prev = line
    return w, h, rows


@check("stickers_have_transparent_corners")
def _():
    for i in of("sticker"):
        w, h, rows = rgba_rows(committed / i["file"])
        assert (w, h) == (1024, 1024) == (i["width"], i["height"]), f"{i['id']}: {w} x {h}"
        for y0, x0 in [(0, 0), (0, w - 32), (h - 32, 0), (h - 32, w - 32)]:
            corner = [rows[y][x * 4 + 3] for y in range(y0, y0 + 32) for x in range(x0, x0 + 32)]
            assert max(corner) == 0, f"{i['id']}: the corner at {x0}, {y0} isn't transparent"
        alpha = [row[x * 4 + 3] for row in rows[::8] for x in range(0, w, 8)]
        solid = sum(a == 255 for a in alpha) / len(alpha)
        assert 0.05 < solid < 0.8, f"{i['id']}: {solid:.0%} of it is solid"


sys.exit(1 if failed else 0)
PY
