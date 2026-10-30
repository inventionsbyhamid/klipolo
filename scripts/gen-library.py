#!/usr/bin/env python3
"""Klipolo's bundled pack: sound effects, music beds and stickers, all made here from code and dedicated to the public
domain (CC0 1.0). No recordings, samples or outside media go in.

Python 3 standard library only, and deterministic: fixed seeds, no clocks, and nothing but IEEE arithmetic and square
roots (sines and exponentials are computed here, never by the platform's maths library), so every machine writes the
same bytes. It writes, into assets/library or --out DIR:
  sounds/<id>.wav     48 kHz 16-bit mono, each under 3 s, starting and ending at silence
  music/<id>.wav      24 kHz 16-bit mono beds of 16 s, drawn exactly periodic so they loop without a click
  stickers/<id>.png   1024 x 1024 RGBA, the shape with a border on a transparent canvas
  manifest.json       each item's id, name, category, file, length or size, tags, sha256 and licence
  LICENSE.txt         the CC0 dedication
Usage: python3 scripts/gen-library.py [--out DIR]
scripts/gen-library.test.sh checks that two runs write the same bytes as the committed assets/library.
"""

import argparse
import hashlib
import io
import json
import math
import pathlib
import struct
import wave
import zlib

PI = 3.141592653589793
TAU = 2 * PI
LN2 = 0.6931471805599453
SQRT2 = math.sqrt(2.0)

# ---------------------------------------------------------------------------------------------------------- maths


def sin(x):
    """sin(x) to about 1e-11: reduced to [-pi/2, pi/2], then its Taylor series to x^15 (Horner)."""
    x -= round(x / TAU) * TAU
    if x > PI / 2:
        x = PI - x
    elif x < -PI / 2:
        x = -PI - x
    x2 = x * x
    return x * (1 - x2 / 6 * (1 - x2 / 20 * (1 - x2 / 42 * (1 - x2 / 72 * (1 - x2 / 110 * (1 - x2 / 156 * (
        1 - x2 / 210)))))))


def cos(x):
    return sin(x + PI / 2)


def exp(x):
    """e^x: 2^k e^r with |r| <= ln 2 / 2, e^r by its Taylor series to r^14."""
    k = round(x / LN2)
    r = x - k * LN2
    total, term = 1.0, 1.0
    for n in range(1, 15):
        term = term * r / n
        total += term
    return math.ldexp(total, k)


def clamp(x, lo, hi):
    return lo if x < lo else hi if x > hi else x


TABLE_SIZE = 8192
SINE = [sin(TAU * i / TABLE_SIZE) for i in range(TABLE_SIZE + 1)]


def osc(phase):
    """sin(2 pi phase) for a phase in cycles, from a table with linear interpolation (error under 3e-7)."""
    phase -= math.floor(phase)
    at = phase * TABLE_SIZE
    i = int(at)
    return SINE[i] + (SINE[i + 1] - SINE[i]) * (at - i)


class Noise:
    """White noise in [-1, 1) from a 32-bit xorshift: the same numbers on every machine."""

    def __init__(self, seed):
        self.state = seed & 0xFFFFFFFF or 1

    def __call__(self):
        s = self.state
        s ^= (s << 13) & 0xFFFFFFFF
        s ^= s >> 17
        s ^= (s << 5) & 0xFFFFFFFF
        self.state = s
        return s / 2147483648.0 - 1.0


class Svf:
    """A state-variable filter (Chamberlin): low, band and high pass at once, its cutoff settable every sample."""

    def __init__(self, rate):
        self.rate, self.low, self.band = rate, 0.0, 0.0

    def __call__(self, x, cutoff, damping):
        f = 2 * sin(PI * cutoff / self.rate)
        high = x - self.low - damping * self.band
        self.band += f * high
        self.low += f * self.band
        return self.low, self.band, high


def swell(t, peak):
    """0 at t = 0, 1 at `peak`, 0 again at t = 1, smooth at both ends."""
    if t < peak:
        s = sin(PI / 2 * t / peak)
    else:
        s = cos(PI / 2 * (t - peak) / (1 - peak))
    return s * s


def note(name):
    """A note's frequency in Hz (A4 = 440, equal temperament): "C4", "F#3"."""
    steps = {"C": -9, "D": -7, "E": -5, "F": -4, "G": -2, "A": 0, "B": 2}[name[0]]
    steps += name.count("#") - name.count("b")
    octave = int(name.lstrip("CDEFGAB#b"))
    return 440.0 * exp((steps + 12 * (octave - 4)) * LN2 / 12)


# ---------------------------------------------------------------------------------------------------------- audio

SR = 48000


def edges(samples, fade_in, fade_out, rate=SR):
    """Linear fades over the first and last seconds given, so the first and last samples are silence."""
    n = len(samples)
    a, b = max(1, int(fade_in * rate)), max(1, int(fade_out * rate))
    for i in range(min(a, n)):
        samples[i] *= i / a
    for i in range(min(b, n)):
        samples[n - 1 - i] *= i / b
    return samples


def normalised(samples, peak):
    top = max(abs(v) for v in samples)
    return [v * peak / top for v in samples]


def wav_bytes(samples, rate):
    """16-bit mono WAV of `samples` in [-1, 1]."""
    frames = struct.pack(f"<{len(samples)}h", *(int(round(clamp(v, -1.0, 1.0) * 32767)) for v in samples))
    out = io.BytesIO()
    with wave.open(out, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(frames)
    return out.getvalue()


def decay(tau, rate=SR):
    """The factor a sample's envelope is multiplied by to fall to 1/e over `tau` seconds."""
    return exp(-1.0 / (tau * rate))


def whoosh(seconds=0.9, peak=0.45, low=250.0, high=2650.0, damping=0.5, seed=11):
    """Air rushing past: noise through two band passes in a row, which sweep up and back down together as the sound
    swells and fades, over a little of its low rumble."""
    n = int(seconds * SR)
    noise, first, second, out = Noise(seed), Svf(SR), Svf(SR), []
    for i in range(n):
        env = swell(i / n, peak)
        cutoff = low + (high - low) * env
        lo, band, _ = first(noise(), cutoff, damping)
        _, band, _ = second(band, cutoff, damping)
        out.append((band + lo * 0.15) * env)
    return out


def pop():
    """A soft pop: a sine whose pitch drops fast, with a short decay."""
    n, out = int(0.16 * SR), []
    phase, sweep, level = 0.0, 1.0, 1.0
    sweep_k, level_k = decay(0.012), decay(0.045)
    for i in range(n):
        phase += (180.0 + 720.0 * sweep) / SR
        out.append(osc(phase) * level * min(1.0, i / (0.0015 * SR)))
        sweep *= sweep_k
        level *= level_k
    return out


def click():
    """A small tick: a short 2.6 kHz ping and a breath of bright noise."""
    n, out, noise = int(0.05 * SR), [], Noise(5)
    tone, hiss, previous = 1.0, 1.0, 0.0
    tone_k, hiss_k = decay(0.0035), decay(0.0015)
    for i in range(n):
        x = noise()
        out.append((osc(2600.0 * i / SR) * tone + (x - previous) * 0.35 * hiss) * min(1.0, i / (0.0003 * SR)))
        previous = x
        tone *= tone_k
        hiss *= hiss_k
    return out


def bell(out, start, freq, partials, level=1.0, attack=0.002, detune=0.0):
    """Adds a struck bell at `start` seconds: `partials` are (ratio, amplitude, decay seconds); `detune` adds a quiet
    copy that many Hz higher, which shimmers against it."""
    begin = int(start * SR)
    for ratio, amp, tau in partials:
        k = decay(tau)
        for f, a in [(freq * ratio, amp)] + ([(freq * ratio + detune, amp * 0.5)] if detune else []):
            env = level * a
            for j in range(len(out) - begin):
                out[begin + j] += osc(f * j / SR) * env * min(1.0, j / (attack * SR))
                env *= k
                if env < 1e-5:
                    break


def ding():
    """A clear notification bell on D6 with inharmonic partials."""
    out = [0.0] * int(1.6 * SR)
    bell(out, 0.0, note("D6"), [(1.0, 1.0, 0.7), (2.0, 0.32, 0.35), (2.76, 0.22, 0.25), (4.07, 0.08, 0.12)])
    return out


def chime():
    """Three soft bells rising through C major, each with a slow shimmer."""
    out = [0.0] * int(2.4 * SR)
    for k, name in enumerate(["C6", "E6", "G6"]):
        bell(out, 0.12 * k, note(name), [(1.0, 1.0, 0.8), (2.0, 0.18, 0.4), (3.0, 0.06, 0.2)], 0.8, 0.004, 1.2)
    return out


def rise():
    """A riser: a bright tone gliding up two octaves under noise that opens up, both growing to the end."""
    n, out, noise, svf = int(1.8 * SR), [], Noise(23), Svf(SR)
    phase = 0.0
    for i in range(n):
        t = i / n
        phase += 110.0 * exp(2 * t * LN2) / SR
        tone = sum(osc(phase * h) / h for h in range(1, 7)) * 0.5
        lo, _, _ = svf(noise(), 300.0 + 5700.0 * t * t, 0.7)
        out.append(tone * t * t + lo * 0.7 * t * t * t)
    return out


def sparkle():
    """A trail of tiny high bells on a pentatonic scale, scattered over the first second and dying away."""
    out, noise = [0.0] * int(1.4 * SR), Noise(42)
    scale = [note(n) for n in ["C7", "D7", "E7", "G7", "A7", "C8"]]
    times = sorted(0.85 * (noise() + 1) / 2 for _ in range(16))
    for k, start in enumerate(times):
        freq = scale[int((noise() + 1) / 2 * len(scale)) % len(scale)]
        level = (1.0 - 0.55 * k / len(times)) * (0.7 + 0.3 * (noise() + 1) / 2)
        tau = 0.09 + 0.11 * (noise() + 1) / 2
        bell(out, start, freq, [(1.0, 1.0, tau), (2.0, 0.25, tau / 2)], level, 0.001)
    return out


# ---------------------------------------------------------------------------------------------------------- music

MUSIC_RATE = 24000
LOOP = 16  # seconds; every repeating part divides it, so the bed's end runs into its start
LOOP_N = LOOP * MUSIC_RATE


def loop_steps(freq):
    """`freq` in 1/16 Hz steps (rounded): a sine of a whole number of them repeats exactly every 16 s."""
    return round(freq * LOOP)


def loop_osc(i, steps, phase=0.0):
    """sin(2 pi (f t + phase)) at sample i of the loop, for f = `steps` / 16 Hz."""
    return osc((i * steps % LOOP_N) / LOOP_N + phase)


def seam(samples):
    """A loop's seam at a zero crossing. A bed is drawn exactly periodic (every sine a whole number of cycles in 16 s,
    every tail carried round to the start), so its end already runs into its start; it is only turned, by at most
    10 ms, to start at the zero crossing nearest its start. No fade: one would put a kink, a click, in the join."""
    n, best = len(samples), None
    for i in range(-240, 241):
        a, b = samples[(i - 1) % n], samples[i % n]
        if (a < 0) != (b < 0) or b == 0:
            score = (abs(b), abs(i))
            if best is None or score < best[0]:
                best = (score, i)
    turn = best[1] % n if best else 0
    return samples[turn:] + samples[:turn]


AMBIENT_CHORDS = [
    ["C3", "G3", "B3", "D4", "E4"],
    ["A2", "E3", "G3", "B3", "C4"],
    ["F2", "C3", "E3", "G3", "A3"],
    ["G2", "D3", "A3", "B3", "E4"],
]


def soft_ambient():
    """A calm pad: four chords (Cmaj9, Am9, Fmaj9, G6/9) melting into each other every 4 s, each note with a soft
    octave and a slowly beating twin, breathing every 8 s, and a faint high shimmer. Each sine starts at its own fixed
    phase: started together, they would all line up into a soft thump at the top of every loop."""
    noise = Noise(3)

    def voice(f):
        return loop_steps(f), (noise() + 1) / 2

    chords = [
        [(voice(f), voice(2 * f), (loop_steps(f) + 4, (noise() + 1) / 2)) for f in map(note, chord)]
        for chord in AMBIENT_CHORDS
    ]
    shimmer = [voice(note(n)) for n in ["E5", "G5", "B5"]]
    out = []
    for i in range(LOOP_N):
        t = i / MUSIC_RATE
        total = 0.0
        for c, chord in enumerate(chords):
            d = (t - 4 * c + LOOP / 2) % LOOP - LOOP / 2  # distance to the chord's centre, around the loop
            if abs(d) >= 4:
                continue
            w = cos(PI * d / 8)
            for one, octave, twin in chord:  # the twin is 0.25 Hz higher: a slow beat
                total += w * w * (loop_osc(i, *one) + 0.22 * loop_osc(i, *octave) + 0.4 * loop_osc(i, *twin)) / 5
        for k, f in enumerate(shimmer):
            total += 0.05 * (0.5 + 0.5 * sin(TAU * (t / 4 + k / 3))) * loop_osc(i, *f)
        out.append(total * (0.85 + 0.15 * sin(TAU * t / 8)))
    return seam(normalised(out, 0.7))


UPBEAT_BARS = [  # bass root, then the chord's tones for the arpeggio and pad
    ("C2", ["C4", "E4", "G4"]),
    ("G2", ["B3", "D4", "G4"]),
    ("A2", ["A3", "C4", "E4"]),
    ("F2", ["A3", "C4", "F4"]),
] * 2


def upbeat_loop():
    """A bright 120 BPM groove over I-V-vi-IV in C, twice: a four-on-the-floor kick, claps on 2 and 4, off-beat hats,
    a plucked eighth-note bass and arpeggio, and a soft pad. Drawn around a 16 s circle, so each tail runs on into the
    loop's start."""
    out, noise = [0.0] * LOOP_N, Noise(7)
    beat = MUSIC_RATE // 2

    def add(start, length, voice):
        for j in range(length):
            out[(start + j) % LOOP_N] += voice(j, j / MUSIC_RATE)

    def kick(j, t):  # its pitch falls from 160 Hz to 50 Hz; the phase is that pitch's integral
        return 0.9 * osc(50.0 * t + 110.0 * 0.03 * (1 - exp(-t / 0.03))) * exp(-t / 0.16) * min(1.0, j / 24)

    for b in range(32):
        start = b * beat
        add(start, int(0.35 * MUSIC_RATE), kick)
        if b % 2 == 1:
            svf = Svf(MUSIC_RATE)

            def clap(j, t, svf=svf):
                _, band, _ = svf(noise(), 1600.0, 0.4)
                burst = max(exp(-max(0.0, t - tap) / 0.012) for tap in (0.0, 0.008, 0.016) if t >= tap)
                return 0.5 * band * (burst * 0.6 + exp(-t / 0.06) * 0.4)

            add(start, int(0.25 * MUSIC_RATE), clap)
        previous = [0.0]

        def hat(j, t):
            x = noise()
            y, previous[0] = x - previous[0], x
            return 0.12 * y * exp(-t / 0.025)

        add(start + beat // 2, int(0.08 * MUSIC_RATE), hat)

    for bar, (root, tones) in enumerate(UPBEAT_BARS):
        bass_f = note(root)
        arp = [note(n) * 2 for n in tones] + [note(tones[0]) * 4]
        for e in range(8):
            start = bar * 4 * beat + e * beat // 2

            def bass(j, t):
                p = bass_f * t
                return 0.42 * (osc(p) + 0.35 * osc(2 * p) + 0.12 * osc(3 * p)) * exp(-t / 0.12) * min(1.0, j / 72)

            def pluck(j, t, f=arp[[0, 1, 2, 3, 2, 1, 0, 1][e]]):
                return 0.2 * (osc(f * t) + 0.15 * osc(3 * f * t)) * exp(-t / 0.18) * min(1.0, j / 48)

            add(start, int(0.24 * MUSIC_RATE), bass)
            add(start, int(0.4 * MUSIC_RATE), pluck)

    pads = [[loop_steps(note(n)) for n in tones] for _, tones in UPBEAT_BARS]
    for i in range(LOOP_N):
        t = i / MUSIC_RATE
        for bar, chord in enumerate(pads):
            d = (t - 2 * bar - 1 + LOOP / 2) % LOOP - LOOP / 2
            if abs(d) < 2:
                w = cos(PI * d / 4)
                out[i] += 0.05 * w * w * sum(loop_osc(i, f) for f in chord)
    return seam(normalised(out, 0.8))


# ---------------------------------------------------------------------------------------------------------- stickers

SIZE = 1024
WHITE = (255, 255, 255)
NAVY = (0x12, 0x1A, 0x33)
BORDER = 28.0  # the white border around each shape, in pixels


def length(x, y):
    return math.sqrt(x * x + y * y)


def segment(px, py, ax, ay, bx, by):
    """Distance from p to the segment a-b."""
    ex, ey, wx, wy = bx - ax, by - ay, px - ax, py - ay
    h = clamp((wx * ex + wy * ey) / (ex * ex + ey * ey), 0.0, 1.0)
    return length(wx - ex * h, wy - ey * h)


def polygon(px, py, points):
    """Signed distance from p to a closed polygon: negative inside."""
    dx, dy = px - points[0][0], py - points[0][1]
    d = dx * dx + dy * dy
    s = 1.0
    j = len(points) - 1
    for i, (vx, vy) in enumerate(points):
        ux, uy = points[j]
        ex, ey, wx, wy = ux - vx, uy - vy, px - vx, py - vy
        h = clamp((wx * ex + wy * ey) / (ex * ex + ey * ey), 0.0, 1.0)
        bx, by = wx - ex * h, wy - ey * h
        d = min(d, bx * bx + by * by)
        c = (py >= vy, py < uy, ex * wy > ey * wx)
        if all(c) or not any(c):
            s = -s
        j = i
    return s * math.sqrt(d)


def heart(x, y):
    """A heart 773 px wide, as two arcs and a point (y up from its tip)."""
    scale = 640.0
    px, py = abs(x) / scale, (353.0 - y) / scale
    if py + px > 1.0:
        return (length(px - 0.25, py - 0.75) - SQRT2 / 4) * scale
    m = 0.5 * max(px + py, 0.0)
    d = math.sqrt(min(px * px + (py - 1.0) * (py - 1.0), (px - m) * (px - m) + (py - m) * (py - m)))
    return d * scale * ((px > py) - (px < py))


def star(x, y):
    """A five-point star pointing up, its corners rounded by 36 px."""
    r, rounding, inner = 380.0, 36.0, 0.5
    k1x, k1y = 0.809016994375, -0.587785252292
    px, py = abs(x), -y + 0.0955 * r
    for kx, ky in ((k1x, k1y), (-k1x, k1y)):
        dot = max(kx * px + ky * py, 0.0)
        px, py = px - 2 * dot * kx, py - 2 * dot * ky
    px, py = abs(px), py - (r - rounding)
    bax, bay = inner * -k1y, inner * k1x - 1.0
    h = clamp((px * bax + py * bay) / (bax * bax + bay * bay), 0.0, r - rounding)
    side = py * bax - px * bay
    return length(px - bax * h, py - bay * h) * ((side > 0) - (side < 0)) - rounding


ARROW = [(-375, -95), (35, -95), (35, -260), (385, 0), (35, 260), (35, 95), (-375, 95)]


def arrow(x, y):
    """A bold arrow pointing right, its corners rounded by 24 px."""
    return polygon(x + 5, y, ARROW) - 24.0


def ring(x, y):
    """A ring 70 px thick, 660 px across its middle."""
    return abs(length(x, y) - 330.0) - 35.0


def check(x, y):
    """A tick 120 px thick with round ends."""
    x += 10
    return min(segment(x, y, -250, 20, -80, 190), segment(x, y, -80, 190, 270, -190)) - 60.0


TAIL = [(-170, 150), (-40, 150), (-230, 330)]


def bubble(x, y):
    """A speech bubble: a rounded box with a tail at the bottom left."""
    qx, qy = abs(x) - 390 + 110, abs(y + 60) - 250 + 110
    box = length(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0) - 110
    return min(box, polygon(x, y, TAIL) - 12.0)


def png_bytes(pixel, reach):
    """A 1024 x 1024 RGBA PNG (filter none, zlib level 9). `pixel(x, y)` gives a pixel's RGBA from its centre's offset
    from the canvas's middle; pixels further than `reach` from the middle on either axis are left transparent."""
    lo, hi = int(SIZE / 2 - reach), int(SIZE / 2 + reach)
    raw = bytearray()
    for y in range(SIZE):
        raw.append(0)
        if not lo <= y < hi:
            raw.extend(bytes(SIZE * 4))
            continue
        raw.extend(bytes(lo * 4))
        for x in range(lo, hi):
            raw.extend(pixel(x + 0.5 - SIZE / 2, y + 0.5 - SIZE / 2))
        raw.extend(bytes((SIZE - hi) * 4))

    def chunk(kind, data):
        return struct.pack("!I", len(data)) + kind + data + struct.pack("!I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack("!2I5B", SIZE, SIZE, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(
        b"IEND", b"")


def cover(d, edge):
    """How much of a pixel whose centre is `d` px from a shape's edge lies inside `edge` px around it."""
    return clamp(0.5 - (d - edge), 0.0, 1.0)


def layered(shape, layers):
    """A pixel function: `layers` are (edge, colour) from the outside in, each painted where the shape's distance is
    under its edge, antialiased over a pixel."""

    def pixel(x, y):
        d = shape(x, y)
        covers = [cover(d, edge) for edge, _ in layers] + [0.0]
        alpha = covers[0]
        if alpha <= 0.0:
            return b"\0\0\0\0"
        rgb = [0.0, 0.0, 0.0]
        for k, (_, colour) in enumerate(layers):
            share = covers[k] - covers[k + 1]
            for c in range(3):
                rgb[c] += colour[c] * share
        return bytes([int(round(clamp(v / alpha, 0.0, 255.0))) for v in rgb] + [int(round(alpha * 255))])

    return pixel


def hex_rgb(h):
    return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))


def sticker(shape, colour):
    """`shape` filled with `colour`, in a white border."""
    return png_bytes(layered(shape, [(BORDER, WHITE), (0.0, hex_rgb(colour))]), 470)


# ---------------------------------------------------------------------------------------------------------- the pack

SOUNDS = [
    ("whoosh", "Whoosh", ["transition", "topic change", "fast", "air", "swipe"],
     lambda: edges(whoosh(), 0.002, 0.01), 0.5),
    ("swoosh", "Swoosh", ["transition", "quick", "swipe", "air", "reveal"],
     lambda: edges(whoosh(0.5, 0.3, 900.0, 5100.0, 0.45, 29), 0.002, 0.01), 0.45),
    ("pop", "Pop", ["pop", "appear", "bubble", "short", "fun"], lambda: edges(pop(), 0.0005, 0.005), 0.55),
    ("click", "Click", ["click", "tap", "button", "short", "ui"], lambda: edges(click(), 0.0002, 0.004), 0.45),
    ("ding", "Ding", ["bell", "notification", "correct", "bright", "alert"], lambda: edges(ding(), 0.001, 0.03), 0.5),
    ("chime", "Chime", ["chime", "bells", "soft", "magic", "success"], lambda: edges(chime(), 0.001, 0.3), 0.45),
    ("rise", "Rise", ["riser", "build up", "tension", "transition", "intro"], lambda: edges(rise(), 0.01, 0.06), 0.5),
    ("sparkle", "Sparkle", ["sparkle", "magic", "shine", "twinkle", "reveal"],
     lambda: edges(sparkle(), 0.001, 0.3), 0.45),
]

MUSIC = [
    ("soft-ambient", "Soft ambient", ["calm", "ambient", "soft", "background", "chill", "loop"], soft_ambient),
    ("upbeat-loop", "Upbeat loop", ["upbeat", "happy", "energetic", "pop", "background", "loop"], upbeat_loop),
]

STICKERS = [
    ("heart", "Heart", ["heart", "love", "like", "pink"], heart, "#FF4D6D"),
    ("star", "Star", ["star", "favourite", "best", "gold"], star, "#FFC21A"),
    ("arrow", "Arrow", ["arrow", "point", "look", "right", "blue"], arrow, "#4C8DFF"),
    ("circle", "Circle", ["circle", "ring", "highlight", "look here", "orange"], ring, "#FF6A3D"),
    ("check", "Check", ["check", "tick", "done", "yes", "green"], check, "#22C55E"),
]

LICENSE = """Klipolo pack: sound effects, music beds and stickers made from code by scripts/gen-library.py.
No recordings, samples or outside media were used.

To the extent possible under law, Klipolo has waived all copyright and related or neighbouring rights to these files.
They are dedicated to the public domain under CC0 1.0 Universal:
https://creativecommons.org/publicdomain/zero/1.0/
"""


def build():
    """Every file of the pack, by path, and the manifest's items."""
    files, items = {}, []

    def item(id_, name, category, path, data, tags, **size):
        files[path] = data
        items.append(dict(id=id_, name=name, category=category, file=path, **size, tags=tags,
                          sha256=hashlib.sha256(data).hexdigest(), license="CC0-1.0"))

    for id_, name, tags, make, peak in SOUNDS:
        samples = normalised(make(), peak)
        item(id_, name, "sound", f"sounds/{id_}.wav", wav_bytes(samples, SR), tags,
             duration_ms=round(len(samples) * 1000 / SR))
    for id_, name, tags, make in MUSIC:
        samples = make()
        item(id_, name, "music", f"music/{id_}.wav", wav_bytes(samples, MUSIC_RATE), tags,
             duration_ms=round(len(samples) * 1000 / MUSIC_RATE))
    for id_, name, tags, shape, colour in STICKERS:
        item(id_, name, "sticker", f"stickers/{id_}.png", sticker(shape, colour), tags, width=SIZE, height=SIZE)
    speech = png_bytes(layered(bubble, [(BORDER, WHITE), (0.0, NAVY), (-18.0, WHITE)]), 470)
    item("bubble", "Speech bubble", "sticker", "stickers/bubble.png", speech,
         ["speech", "talk", "say", "quote", "white"], width=SIZE, height=SIZE)

    manifest = dict(version=1, id="klipolo-pack", name="Klipolo pack", license="CC0-1.0", creator="Klipolo",
                    items=items)
    files["manifest.json"] = (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode()
    files["LICENSE.txt"] = LICENSE.encode()
    return files


def main():
    parser = argparse.ArgumentParser(description="Write Klipolo's bundled CC0 pack.")
    default = pathlib.Path(__file__).resolve().parents[1] / "assets" / "library"
    parser.add_argument("--out", type=pathlib.Path, default=default, help=f"where to write it (default {default})")
    out = parser.parse_args().out
    files = build()
    for path, data in files.items():
        target = out / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    print(f"Wrote {len(files)} files, {sum(len(d) for d in files.values())} bytes, into {out}")


if __name__ == "__main__":
    main()
