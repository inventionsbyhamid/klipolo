#!/usr/bin/env python3
"""Klipolo's bundled pack: sound effects, music beds, stickers and backgrounds, all made here from code and dedicated to
the public domain (CC0 1.0). No recordings, samples or outside media go in.

Python 3 standard library only, and deterministic: fixed seeds, no clocks, and nothing but IEEE arithmetic and square
roots (sines and exponentials are computed here, never by the platform's maths library), so every machine works out
the same samples and pixels. The PNGs are deflated by the zlib Python is built with, which can differ between builds
(zlib-ng, Apple's), so regenerate the pack in the klipolo-ci image (ci/linux.Dockerfile), where its check runs.
It writes, into assets/library or --out DIR:
  sounds/<id>.wav     48 kHz 16-bit mono, each under 3 s, starting and ending at silence
  music/<id>.wav      24 kHz 16-bit mono beds of 16 s, drawn exactly periodic so they loop without a click
  stickers/<id>.png   1024 x 1024 RGBA, the shape with a border on a transparent canvas
  backgrounds/<id>.png  1920 x 1080 or 1080 x 1920 RGB, sized for a 16:9 or 9:16 frame (listed as stickers: pictures)
  manifest.json       each item's id, name, category, file, length or size, tags, sha256 and licence
  LICENSE.txt         the CC0 dedication
Usage: python3 scripts/gen-library.py [--out DIR] [--workers N]
scripts/gen-library.test.sh checks that two runs write the same bytes as the committed assets/library.
"""

import argparse
import concurrent.futures
import hashlib
import io
import json
import math
import os
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


# ------------------------------------------------------------------------------------------- more sound effects


def finished(samples, fade_in, fade_out, rate=SR):
    """A sound as the pack ships it: its DC offset taken out (a 10 Hz high pass), the near silence before and after it
    (under a thousandth of its peak) trimmed, and short fades so it starts and ends at silence."""
    k = 1.0 - TAU * 10.0 / rate
    out, x1, y1 = [], 0.0, 0.0
    for x in samples:
        y1 = x - x1 + k * y1
        x1 = x
        out.append(y1)
    top = max(abs(v) for v in out)
    heard = [i for i, v in enumerate(out) if abs(v) * 1000 >= top]
    return edges(out[heard[0]:heard[-1] + 1], fade_in, fade_out, rate)


def soft(x):
    """A soft clip: about x when small, never past 1 either way (a rational tanh)."""
    if x >= 3.0:
        return 1.0
    if x <= -3.0:
        return -1.0
    return x * (27.0 + x * x) / (27.0 + 9.0 * x * x)


def unit(noise):
    """The next of `noise`'s numbers as a share in [0, 1)."""
    return (noise() + 1) / 2


def impact():
    """A heavy hit: a sine thump whose pitch falls from 110 Hz to 38 Hz, a burst of noise closing from bright to dull,
    and a low rumble after it, pushed a little into a soft clip for weight."""
    n, noise, deep = int(1.6 * SR), Noise(101), Noise(163)
    burst, rumble = Svf(SR), Svf(SR)
    out, phase = [], 0.0
    glide, body, level, tail = 1.0, 1.0, 1.0, 1.0
    glide_k, body_k, level_k, tail_k = decay(0.07), decay(0.45), decay(0.16), decay(0.6)
    for i in range(n):
        phase += (38.0 + 72.0 * glide) / SR
        lo, _, _ = burst(noise(), 180.0 + 3800.0 * glide, 0.9)
        low, _, _ = rumble(deep(), 90.0, 0.8)
        x = 1.1 * osc(phase) * body + 0.9 * lo * level + 6.0 * low * tail
        out.append(soft(1.6 * x) * min(1.0, i / (0.001 * SR)))
        glide *= glide_k
        body *= body_k
        level *= level_k
        tail *= tail_k
    return finished(out, 0.0005, 0.08)


def bass_drop():
    """A deep drop: a sine gliding from 160 Hz down to 34 Hz over two seconds, warmed by a soft clip so small speakers
    carry it too, with a click of noise as it starts."""
    n, noise, out, phase = int(2.3 * SR), Noise(107), [], 0.0
    glide, click = 1.0, 1.0
    glide_k, click_k = decay(0.55), decay(0.004)
    for i in range(n):
        t = i / SR
        phase += (34.0 + 126.0 * glide) / SR
        fall = 1.0 if t < 1.6 else cos(PI / 2 * (t - 1.6) / 0.7)
        env = min(1.0, i / (0.004 * SR)) * fall * fall
        out.append(0.8 * soft(2.2 * osc(phase)) * env + 0.5 * noise() * click)
        glide *= glide_k
        click *= click_k
    return finished(out, 0.0005, 0.05)


def long_riser():
    """A long build to a moment: noise through a band pass sweeping from 200 Hz to 7 kHz, and two detuned buzzy tones
    gliding up an octave and a half, both growing to the end under a tremolo that speeds up."""
    n, noise, svf, out = int(2.9 * SR), Noise(109), Svf(SR), []
    glide, glide_k = 1.0, exp(1.5 * LN2 / n)
    cutoff, cutoff_k = 200.0, exp(3.5553480614894135 / n)  # 200 Hz times 35
    phases, wobble = [0.0, 0.0], 0.0
    for i in range(n):
        t = i / n
        tone = 0.0
        for k, detune in enumerate((1.0, 1.007)):
            phases[k] += 130.0 * detune * glide / SR
            tone += sum(osc(phases[k] * h) / h for h in range(1, 6))
        wobble += (4.0 + 14.0 * t * t) / SR
        _, band, _ = svf(noise(), cutoff, 0.6)
        tremolo = 1.0 - 0.3 * t * (0.5 + 0.5 * osc(wobble))
        out.append((0.25 * tone + 1.2 * band) * t * t * tremolo)
        glide *= glide_k
        cutoff *= cutoff_k
    return finished(out, 0.001, 0.03)


def swipe(up, seed):
    """A quick swipe: a third of a second of noise through a band pass sweeping from 600 Hz to 4.2 kHz, or back down,
    as it swells and fades."""
    n, noise, svf, out = int(0.32 * SR), Noise(seed), Svf(SR), []
    for i in range(n):
        t = i / n
        _, band, _ = svf(noise(), 600.0 * exp((t if up else 1 - t) * 1.9459101090932196), 0.35)  # up to 7 times
        out.append(band * swell(t, 0.45))
    return finished(out, 0.001, 0.01)


def glitch():
    """A digital glitch: 14 cuts of 15 to 55 ms, each a buzzing square tone, a burst of crushed noise, a gap, or the
    cut before it again."""
    noise, out, last = Noise(149), [], []
    for _ in range(14):
        length = int((0.015 + 0.04 * unit(noise)) * SR)
        kind = int(unit(noise) * 4)
        if kind == 3 and last:
            cut = last[:]
        elif kind in (0, 3):
            r = unit(noise)
            f = 180.0 + 1400.0 * r * r
            cut = [0.5 if osc(f * j / SR) >= 0 else -0.5 for j in range(length)]
        elif kind == 1:
            hold, step, cut = 0.0, 1 + int(unit(noise) * 12), []
            for j in range(length):
                if j % step == 0:
                    hold = round(noise() * 4) / 4
                cut.append(0.7 * hold)
        else:
            cut = [0.0] * (length // 2)
        cut = edges(cut, 0.0005, 0.0005)
        out.extend(cut)
        last = cut
    return finished(out, 0.0005, 0.005)


def record_scratch():
    """A record scratched back and forth and then stopped: a buzzy tone and grainy noise, as if cut in the groove,
    played at the hand's speed, backwards and forwards, louder and brighter the faster it moves."""
    grain, groove, smooth = Noise(113), [], 0.0
    for _ in range(SR):
        smooth += 0.35 * (grain() - smooth)
        groove.append(smooth)
    strokes = [(0.0, 0.11, -6.0), (0.11, 0.2, 5.0), (0.2, 0.3, -6.5), (0.3, 0.62, 3.0)]
    n, svf, out, at = int(0.62 * SR), Svf(SR), [], 0.3
    for i in range(n):
        t = i / SR
        speed = next(v * sin(PI * (t - a) / (b - a)) for a, b, v in strokes if t < b)
        at += speed / SR
        tone = sum(osc(180.0 * h * at) / h for h in range(1, 6))
        where = at * SR % SR
        j = int(where)
        g = groove[j] + (groove[(j + 1) % SR] - groove[j]) * (where - j)
        lo, _, _ = svf(0.5 * tone + 2.0 * g, 300.0 + 2500.0 * min(abs(speed), 2.0), 0.6)
        out.append(lo * min(1.0, abs(speed)))
    return finished(out, 0.001, 0.02)


def shutter():
    """A camera's shutter: two quick mechanical clacks 70 ms apart, each a snap of filtered noise over a small thud,
    with a faint ring of metal."""
    out = [0.0] * int(0.26 * SR)
    for start, level, seed in ((0.0, 1.0, 127), (0.07, 0.75, 129)):
        noise, svf, begin = Noise(seed), Svf(SR), int(start * SR)
        snap, thud, ring = 1.0, 1.0, 1.0
        snap_k, thud_k, ring_k = decay(0.006), decay(0.018), decay(0.012)
        for j in range(len(out) - begin):
            _, band, _ = svf(noise(), 3200.0, 0.5)
            x = 1.2 * band * snap + 0.5 * osc(170.0 * j / SR) * thud + 0.12 * osc(4300.0 * j / SR) * ring
            out[begin + j] += x * level * min(1.0, j / (0.0003 * SR))
            snap *= snap_k
            thud *= thud_k
            ring *= ring_k
    return finished(out, 0.0003, 0.01)


def key_press(out, start, level, centre, body, noise):
    """Adds one key at `start` seconds: a click of noise around `centre` Hz over a short thock at `body` Hz."""
    svf, begin = Svf(SR), int(start * SR)
    click, thock = 1.0, 1.0
    click_k, thock_k = decay(0.004), decay(0.012)
    for j in range(min(int(0.06 * SR), len(out) - begin)):
        _, band, _ = svf(noise(), centre, 0.6)
        out[begin + j] += level * (band * click + 0.45 * osc(body * j / SR) * thock) * min(1.0, j / 12)
        click *= click_k
        thock *= thock_k


def typing():
    """Typing on a keyboard: 13 keys at an uneven pace, each a click over a small thock and a softer click as it comes
    back up; the tenth is a deeper space bar."""
    noise, times, t = Noise(131), [], 0.0
    for k in range(13):
        times.append(t)
        t += 0.085 + 0.09 * unit(noise) + (0.12 if k == 6 else 0.0)
    out = [0.0] * int((times[-1] + 0.2) * SR)
    for k, start in enumerate(times):
        level = 0.75 + 0.25 * unit(noise)
        centre = 1800.0 + 1600.0 * unit(noise)
        body = 150.0 if k == 9 else 260.0 + 120.0 * unit(noise)
        key_press(out, start, level, centre, body, noise)
        key_press(out, start + 0.05 + 0.03 * unit(noise), 0.35 * level, 1.2 * centre, 1.3 * body, noise)
    return finished(out, 0.0003, 0.02)


def notification():
    """A friendly two-note alert, E6 then B6, each a soft marimba-like strike."""
    out = [0.0] * int(1.0 * SR)
    for start, name in ((0.0, "E6"), (0.11, "B6")):
        bell(out, start, note(name), [(1.0, 1.0, 0.22), (3.93, 0.12, 0.03), (2.0, 0.08, 0.08)], 1.0, 0.0015)
    return finished(out, 0.0005, 0.05)


def success():
    """A level-up flourish: C5, E5, G5 and C6 played quickly upwards as bright square-ish plucks, the last held a
    moment longer."""
    out = [0.0] * int(1.3 * SR)
    for k, name in enumerate(["C5", "E5", "G5", "C6"]):
        f, begin = note(name), int(0.075 * k * SR)
        level, level_k = 1.0, decay(0.09 if k < 3 else 0.45)
        for j in range(len(out) - begin):
            t = j / SR
            x = osc(f * t) + 0.25 * osc(2 * f * t) + 0.2 * osc(3 * f * t) + 0.08 * osc(5 * f * t)
            out[begin + j] += 0.6 * x * level * min(1.0, j / (0.002 * SR))
            level *= level_k
            if level < 1e-5:
                break
    return finished(out, 0.0005, 0.06)


def error_buzz():
    """A wrong-answer buzz: two short low buzzes, each two square-ish tones (110 Hz and 117 Hz, odd harmonics to the
    ninth) beating against each other."""
    out = []
    for seconds in (0.2, 0.32):
        buzz = []
        for j in range(int(seconds * SR)):
            t = j / SR
            buzz.append(0.35 * sum(osc(h * f * t) / h for f in (110.0, 117.0) for h in (1, 3, 5, 7, 9)))
        out.extend(edges(buzz, 0.004, 0.012))
        out.extend([0.0] * int(0.07 * SR))
    return finished(out, 0.0005, 0.01)


def popped(out, start, low, rise, level, sweep_tau, level_tau, seconds):
    """Adds a pop at `start` seconds: a sine whose pitch drops fast from `low` + `rise` Hz to `low`."""
    begin, phase, sweep, env = int(start * SR), 0.0, 1.0, level
    sweep_k, level_k = decay(sweep_tau), decay(level_tau)
    for j in range(min(int(seconds * SR), len(out) - begin)):
        phase += (low + rise * sweep) / SR
        out[begin + j] += osc(phase) * env * min(1.0, j / (0.001 * SR))
        sweep *= sweep_k
        env *= level_k


def pop_high():
    """A small bright pop, higher and quicker than Pop."""
    out = [0.0] * int(0.1 * SR)
    popped(out, 0.0, 520.0, 1500.0, 1.0, 0.008, 0.025, 0.1)
    return finished(out, 0.0003, 0.005)


def bubbles():
    """Four quick pops of bubbles, at uneven moments and pitches."""
    out = [0.0] * int(0.42 * SR)
    for start, low, level in ((0.0, 300.0, 1.0), (0.09, 420.0, 0.8), (0.15, 360.0, 0.9), (0.27, 560.0, 0.75)):
        popped(out, start, low, 3.2 * low, level, 0.01, 0.035, 0.14)
    return finished(out, 0.0003, 0.01)


def boing():
    """A cartoon spring: a tone wobbling fast around a pitch that bends up from 150 Hz to 260 Hz, the wobble and the
    tone dying away together."""
    n, out, phase, wobble = int(0.95 * SR), [], 0.0, 0.0
    depth, level, bend = 1.0, 1.0, 1.0
    depth_k, level_k, bend_k = decay(0.3), decay(0.32), decay(0.12)
    for i in range(n):
        wobble += 13.0 / SR
        phase += (260.0 - 110.0 * bend) * (1.0 + 0.22 * depth * osc(wobble)) / SR
        x = osc(phase) + 0.35 * osc(2 * phase) + 0.15 * osc(3 * phase)
        out.append(x * level * min(1.0, i / (0.004 * SR)))
        depth *= depth_k
        level *= level_k
        bend *= bend_k
    return finished(out, 0.0005, 0.04)


def drum_hit():
    """One big drum hit: a kick's thump falling from 150 Hz to 55 Hz under a snare's crack of filtered noise and its
    shell's ring at 190 Hz."""
    n, noise, svf, out, phase = int(0.7 * SR), Noise(151), Svf(SR), [], 0.0
    glide, kick, snare, shell = 1.0, 1.0, 1.0, 1.0
    glide_k, kick_k, snare_k, shell_k = decay(0.03), decay(0.2), decay(0.13), decay(0.07)
    for i in range(n):
        phase += (55.0 + 95.0 * glide) / SR
        _, band, high = svf(noise(), 1900.0, 0.8)
        x = 0.9 * osc(phase) * kick + (0.6 * band + 0.25 * high) * snare + 0.3 * osc(190.0 * i / SR) * shell
        out.append(soft(1.3 * x) * min(1.0, i / (0.0008 * SR)))
        glide *= glide_k
        kick *= kick_k
        snare *= snare_k
        shell *= shell_k
    return finished(out, 0.0003, 0.05)


def tick_tock():
    """A clock: tick, tock, tick, tock, half a second apart, each a woodblock knock (a short ping over a click), the
    tock lower."""
    out, noise = [0.0] * int(1.68 * SR), Noise(157)
    for k in range(4):
        f, begin, svf = 2350.0 if k % 2 == 0 else 1650.0, int(0.5 * k * SR), Svf(SR)
        ping, click = 1.0, 1.0
        ping_k, click_k = decay(0.018), decay(0.002)
        for j in range(int(0.15 * SR)):
            _, band, _ = svf(noise(), 1.4 * f, 0.7)
            out[begin + j] += (0.8 * osc(f * j / SR) * ping + 0.5 * band * click) * min(1.0, j / 10)
            ping *= ping_k
            click *= click_k
    return finished(out, 0.0003, 0.02)


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


def loop_lowpass(samples, cutoff):
    """`samples` through a one-pole low pass as if they had looped for ever: once round to settle it, then again."""
    a, y, out = 1.0 - exp(-TAU * cutoff / MUSIC_RATE), 0.0, []
    for x in samples:
        y += a * (x - y)
    for x in samples:
        y += a * (x - y)
        out.append(y)
    return out


class Bed:
    """A bed being drawn around the loop: each voice added at a sample, its tail running on into the loop's start."""

    def __init__(self, seed):
        self.out, self.noise, self.r = [0.0] * LOOP_N, Noise(seed), MUSIC_RATE

    def kick(self, start, level, high=110.0, low=45.0, tau=0.22):
        phase, glide, env = 0.0, 1.0, level
        glide_k, env_k = decay(0.035, self.r), decay(tau, self.r)
        for j in range(int(5 * tau * self.r)):
            phase += (low + (high - low) * glide) / self.r
            self.out[(start + j) % LOOP_N] += osc(phase) * env * min(1.0, j / 24)
            glide *= glide_k
            env *= env_k

    def snare(self, start, level, tau=0.12):
        """Noise around 1.7 kHz over a short 185 Hz body."""
        svf, env, body = Svf(self.r), level, level
        env_k, body_k = decay(tau, self.r), decay(0.06, self.r)
        for j in range(int(3 * tau * self.r)):
            _, band, _ = svf(self.noise(), 1700.0, 0.7)
            x = 0.9 * band * env + 0.4 * osc(185.0 * j / self.r) * body
            self.out[(start + j) % LOOP_N] += x * min(1.0, j / 24)
            env *= env_k
            body *= body_k

    def clap(self, start, level):
        """Three quick bursts of noise around 1.6 kHz, then a short tail."""
        svf = Svf(self.r)
        for j in range(int(0.25 * self.r)):
            t = j / self.r
            _, band, _ = svf(self.noise(), 1600.0, 0.4)
            burst = max(exp(-max(0.0, t - tap) / 0.012) for tap in (0.0, 0.008, 0.016) if t >= tap)
            self.out[(start + j) % LOOP_N] += level * band * (0.6 * burst + 0.4 * exp(-t / 0.06))

    def hat(self, start, level, tau=0.025):
        """Noise with its lows taken out (each sample less the one before), dying away over `tau` seconds."""
        before, env, env_k = 0.0, level, decay(tau, self.r)
        for j in range(int(4 * tau * self.r)):
            x = self.noise()
            self.out[(start + j) % LOOP_N] += (x - before) * env
            before = x
            env *= env_k

    def tone(self, start, length, freq, level, partials, tau, attack=0.005, release=0.04):
        """A note of `partials` ((ratio, amplitude)), dying away over `tau` seconds, held `length` samples."""
        env, env_k, rise, fall = level, decay(tau, self.r), attack * self.r, release * self.r
        for j in range(length + int(fall)):
            t = j / self.r
            x = sum(a * osc(freq * ratio * t) for ratio, a in partials)
            gate = 1.0 if j < length else 1.0 - (j - length) / fall
            self.out[(start + j) % LOOP_N] += x * env * gate * min(1.0, j / rise)
            env *= env_k

    def keys(self, start, length, freq, level):
        """An electric piano note: a sine bent by a fainter sine of its own pitch, brighter as it is struck, with a
        faint metal tine."""
        env, bright, tine = level, 1.0, 1.0
        env_k, bright_k, tine_k = decay(1.4, self.r), decay(0.35, self.r), decay(0.03, self.r)
        fall = int(0.25 * self.r)
        for j in range(length + fall):
            p = freq * j / self.r
            x = osc(p + 0.22 * bright * osc(p)) + 0.06 * tine * osc(14.0 * p)
            gate = 1.0 if j < length else 1.0 - (j - length) / fall
            self.out[(start + j) % LOOP_N] += x * env * gate * min(1.0, j / 72)
            env *= env_k
            bright *= bright_k
            tine *= tine_k


CHILL_BARS = [  # 90 BPM, a bar each: the bass's root, then the electric piano's chord
    ("F2", ["A3", "C4", "E4", "G4"]),
    ("E2", ["G3", "B3", "D4", "F#4"]),
    ("D2", ["F3", "A3", "C4", "E4"]),
    ("C2", ["E3", "G3", "B3", "D4"]),
    ("F2", ["A3", "C4", "E4", "G4"]),
    ("G2", ["B3", "D4", "E4", "A4"]),
]


def chill_beat():
    """A calm lo-fi groove at 90 BPM over six bars (Fmaj9, Em9, Dm9, Cmaj9, Fmaj9, G6/9): a soft electric piano over
    a softer pad, a round bass, a lazy swung kick, snare and hats, and the crackle of a record, all a little dull, as if
    from tape."""
    bed = Bed(17)
    beat = LOOP_N // 24
    swing = beat // 12
    for b, (root, chord) in enumerate(CHILL_BARS):
        start = b * 4 * beat
        for at, level in ((0, 0.8), (5 * beat // 2, 0.7)):
            bed.kick(start + at, level)
        if b % 2 == 1:
            bed.kick(start + 7 * beat // 2, 0.45)
        for at in (beat, 3 * beat):
            bed.snare(start + at, 0.32)
        for e in range(8):
            bed.hat(start + e * beat // 2 + (swing if e % 2 else 0), 0.05 if e % 2 == 0 else 0.032)
        bass = note(root)
        bed.tone(start, 3 * beat // 2, bass, 0.34, [(1.0, 1.0), (2.0, 0.25)], 0.8, 0.01, 0.06)
        bed.tone(start + 5 * beat // 2, 5 * beat // 4, bass, 0.28, [(1.0, 1.0), (2.0, 0.25)], 0.8, 0.01, 0.06)
        for k, name in enumerate(chord):
            bed.keys(start + k * 360, 5 * beat // 2 - k * 360, note(name), 0.12)
            bed.keys(start + 5 * beat // 2 + k * 240, 3 * beat // 2 - k * 240, note(name), 0.08)
    for _ in range(70):  # the record's crackle: small pops anywhere round the loop
        at, level = int(unit(bed.noise) * LOOP_N), 0.02 + 0.06 * unit(bed.noise)
        for j in range(6):
            bed.out[(at + j) % LOOP_N] += level * bed.noise() * (1 - j / 6)
    bar, pads = 4 * beat, [[loop_steps(note(n)) for n in chord] for _, chord in CHILL_BARS]
    for i in range(LOOP_N):  # a soft pad of each bar's chord under it all
        j = i % bar
        bed.out[i] += 0.025 * min(1.0, j / 1200, (bar - j) / 1200) * sum(loop_osc(i, f) for f in pads[i // bar])
    hiss = [x + 0.004 * bed.noise() for x in bed.out]
    return seam(normalised(loop_lowpass(hiss, 3800.0), 0.7))


ENERGY_BARS = [  # 120 BPM, a bar each: the bass's root, then the pad's chord
    ("A1", ["A3", "C4", "E4"]),
    ("F1", ["A3", "C4", "F4"]),
    ("C2", ["G3", "C4", "E4"]),
    ("G1", ["G3", "B3", "D4"]),
] * 2


def energetic_loop():
    """A driving 120 BPM groove in A minor (Am, F, C, G, twice): a punchy four-on-the-floor kick, claps on 2 and 4,
    sixteenth hats with open ones off the beat, a buzzy off-beat bass, a bright pad that ducks under every kick, plucked
    sixteenth arpeggios, and a snare roll and a rush of noise building through the last bar."""
    bed = Bed(23)
    beat, bar = MUSIC_RATE // 2, 2 * MUSIC_RATE
    sixteenth = beat // 4
    saw = [(1.0, 1.0), (2.0, 0.5), (3.0, 0.33), (4.0, 0.25), (5.0, 0.2), (6.0, 0.16)]
    square = [(1.0, 1.0), (3.0, 0.33), (5.0, 0.2)]
    for b, (root, chord) in enumerate(ENERGY_BARS):
        start = b * bar
        for k in range(4):
            bed.kick(start + k * beat, 0.9, 160.0, 48.0, 0.18)
            bed.tone(start + k * beat + beat // 2, beat // 3, 2 * note(root), 0.2, saw, 0.12, 0.003, 0.02)
            bed.tone(start + k * beat + beat // 2, beat // 3, note(root), 0.3, [(1.0, 1.0)], 0.15, 0.003, 0.02)
            bed.hat(start + k * beat + beat // 2, 0.06, 0.1)
        for k in (1, 3):
            bed.clap(start + k * beat, 0.45)
        tones = [note(n) * 2 for n in chord] + [note(chord[0]) * 4]
        for s in range(16):
            if s % 2 == 0 or s % 4 == 3:
                bed.hat(start + s * sixteenth, 0.045 if s % 4 == 0 else 0.03)
            if b < 7:
                bed.tone(start + s * sixteenth, sixteenth, tones[[0, 1, 2, 3, 2, 1, 2, 3][s % 8]], 0.07, square, 0.08,
                         0.002, 0.02)
            else:
                bed.snare(start + s * sixteenth, 0.08 + 0.3 * s / 15, 0.05)
    out = bed.out
    pads = [[loop_steps(note(n)) for n in chord] for _, chord in ENERGY_BARS]
    noise, svf = Noise(29), Svf(MUSIC_RATE)
    for i in range(LOOP_N):
        b, j = i // bar, i % bar
        into = (j % beat) / MUSIC_RATE
        pump = 1.0 - 0.75 * exp(-into / 0.11)
        edge = min(1.0, j / 720, (bar - j) / 720)
        pad = sum(loop_osc(i, f) + loop_osc(i, f + 2) + 0.5 * loop_osc(i, 2 * f) + 0.33 * loop_osc(i, 3 * f)
                  for f in pads[b])
        out[i] += 0.045 * pad * pump * edge
        if b == 7:  # a riser of noise through the last bar
            t = j / bar
            _, band, _ = svf(noise(), 500.0 + 5500.0 * t * t, 0.5)
            out[i] += 0.25 * band * t * t * edge
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
    return png_file(SIZE, SIZE, 6, raw)


def png_file(width, height, colour, raw):
    """A PNG of `raw` (each row its filter byte, then its pixels), 8 bits a channel, of PNG colour type `colour` (2 RGB,
    6 RGBA), at zlib level 9."""

    def chunk(kind, data):
        return struct.pack("!I", len(data)) + kind + data + struct.pack("!I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack("!2I5B", width, height, 8, colour, 0, 0, 0)
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


# ----------------------------------------------------------------------------------------------- more stickers
#
# Drawn as layers from the bottom up: a shape, the distance from it its edge is at, and a colour painted over what is
# under it. A shape is a signed distance (negative inside) that never changes faster than the point it is measured at
# moves, so a square of pixels whose middle is far enough from every layer's edge is one colour throughout and is
# painted at once; only the squares an edge crosses are worked out pixel by pixel.

RED = hex_rgb("#FF3B30")
BLUE = hex_rgb("#4C8DFF")
GOLD = hex_rgb("#FFC21A")
HILITE = hex_rgb("#FFD02E")
ORANGE = hex_rgb("#FF6A1F")
PURPLE = hex_rgb("#8B5CF6")
GREY = hex_rgb("#9AA5B8")
OUTLINE = 18.0  # the navy outline inside a white shape's border, in pixels


class Shape:
    """A signed distance from a point to a shape's edge, negative inside: called with the point's x and y."""

    def near(self, x, y, reach):
        """The shape as seen from points within `reach` of (x, y): the same distances, often with less to work out."""
        return self


class Circle(Shape):
    def __init__(self, cx, cy, r):
        self.cx, self.cy, self.r = cx, cy, r

    def __call__(self, x, y):
        return length(x - self.cx, y - self.cy) - self.r


class Box(Shape):
    """A box `2 hw` x `2 hh` around (cx, cy), its corners rounded by `r`."""

    def __init__(self, cx, cy, hw, hh, r):
        self.cx, self.cy, self.hw, self.hh, self.r = cx, cy, hw - r, hh - r, r

    def __call__(self, x, y):
        qx, qy = abs(x - self.cx) - self.hw, abs(y - self.cy) - self.hh
        return length(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0) - self.r


class Taper(Shape):
    """A round-ended line from a (radius ra) to b (radius rb), its sides straight."""

    def __init__(self, ax, ay, ra, bx, by, rb):
        h = length(bx - ax, by - ay)
        self.ax, self.ay, self.ux, self.uy, self.h, self.ra, self.rb = ax, ay, (bx - ax) / h, (by - ay) / h, h, ra, rb
        self.b = (ra - rb) / h
        self.a = math.sqrt(1.0 - self.b * self.b)

    def __call__(self, x, y):
        wx, wy = x - self.ax, y - self.ay
        qx, qy = abs(wx * self.uy - wy * self.ux), wx * self.ux + wy * self.uy
        k = -self.b * qx + self.a * qy
        if k < 0.0:
            return length(qx, qy) - self.ra
        if k > self.a * self.h:
            return length(qx, qy - self.h) - self.rb
        return qx * self.a + qy * self.b - self.ra


class Polygon(Shape):
    def __init__(self, points, rounding=0.0):
        self.points, self.rounding = points, rounding

    def __call__(self, x, y):
        return polygon(x, y, self.points) - self.rounding


def nearest(segments, x, y):
    """The distance from (x, y) to the nearest of `segments` (ax, ay, ex, ey, 1 / |e|^2)."""
    best = 1e30
    for ax, ay, ex, ey, inv in segments:
        wx, wy = x - ax, y - ay
        h = (wx * ex + wy * ey) * inv
        h = 0.0 if h < 0.0 else 1.0 if h > 1.0 else h
        dx, dy = wx - ex * h, wy - ey * h
        d = dx * dx + dy * dy
        if d < best:
            best = d
    return math.sqrt(best)


class Strokes(Shape):
    """Round-ended lines `width` px wide along polylines (lists of points; one point alone is a dot)."""

    def __init__(self, lines, width, segments=None):
        self.half = width / 2
        if segments is None:
            segments = []
            for line in lines:
                points = line if len(line) > 1 else line * 2
                for (ax, ay), (bx, by) in zip(points, points[1:]):
                    ex, ey = bx - ax, by - ay
                    segments.append((ax, ay, ex, ey, 1.0 / (ex * ex + ey * ey) if ex or ey else 0.0))
        self.segments = segments

    def __call__(self, x, y):
        return nearest(self.segments, x, y) - self.half

    def near(self, x, y, reach):
        far = [nearest([s], x, y) for s in self.segments]
        least = min(far)
        return Strokes(None, 2 * self.half, [s for s, d in zip(self.segments, far) if d <= least + 2 * reach + 0.01])


class Union(Shape):
    """Every part together; with `k`, blended into each other over k px where they meet."""

    def __init__(self, *parts, k=0.0):
        self.parts, self.k = parts, k

    def __call__(self, x, y):
        if not self.k:
            return min(p(x, y) for p in self.parts)
        d = None
        for p in self.parts:  # a polynomial smooth minimum, a part at a time
            v = p(x, y)
            if d is None:
                d = v
            else:
                h = max(self.k - abs(d - v), 0.0) / self.k
                d = min(d, v) - h * h * self.k / 4
        return d

    def near(self, x, y, reach):
        if self.k:  # blended, every part counts
            return Union(*[p.near(x, y, reach) for p in self.parts], k=self.k)
        values = [p(x, y) for p in self.parts]
        least = min(values)
        kept = [p.near(x, y, reach) for p, v in zip(self.parts, values) if v <= least + 2 * reach + 0.01]
        return kept[0] if len(kept) == 1 else Union(*kept)


class Smooth(Shape):
    """Two shapes blended into each other over `k` px where they meet (a polynomial smooth minimum)."""

    def __init__(self, a, b, k):
        self.a, self.b, self.k = a, b, k

    def __call__(self, x, y):
        a, b = self.a(x, y), self.b(x, y)
        h = max(self.k - abs(a - b), 0.0) / self.k
        return min(a, b) - h * h * self.k / 4

    def near(self, x, y, reach):
        a, b = self.a(x, y), self.b(x, y)
        if a > b + 2 * reach + self.k + 0.01:
            return self.b.near(x, y, reach)
        if b > a + 2 * reach + self.k + 0.01:
            return self.a.near(x, y, reach)
        return Smooth(self.a.near(x, y, reach), self.b.near(x, y, reach), self.k)


class Within(Shape):
    """Where every part overlaps (the largest of their distances); with `k`, its corners rounded over k px."""

    def __init__(self, *parts, k=0.0):
        self.parts, self.k = parts, k

    def __call__(self, x, y):
        if not self.k:
            return max(p(x, y) for p in self.parts)
        d = None
        for p in self.parts:  # a polynomial smooth maximum, a part at a time
            v = p(x, y)
            if d is None:
                d = v
            else:
                h = max(self.k - abs(d - v), 0.0) / self.k
                d = max(d, v) + h * h * self.k / 4
        return d


class Outside(Shape):
    """Everything but `shape`."""

    def __init__(self, shape):
        self.shape = shape

    def __call__(self, x, y):
        return -self.shape(x, y)


class Grown(Shape):
    """`shape` grown by `by` px all round (or shrunk, for less than 0)."""

    def __init__(self, shape, by):
        self.shape, self.by = shape, by

    def __call__(self, x, y):
        return self.shape(x, y) - self.by

    def near(self, x, y, reach):
        return Grown(self.shape.near(x, y, reach), self.by)


class Turned(Shape):
    """`shape` turned by `degrees` (clockwise on the canvas) about the canvas's middle."""

    def __init__(self, shape, degrees, cs=None):
        self.shape = shape
        self.c, self.s = cs if cs else (cos(degrees * PI / 180), sin(degrees * PI / 180))

    def __call__(self, x, y):
        return self.shape(self.c * x + self.s * y, -self.s * x + self.c * y)

    def near(self, x, y, reach):
        return Turned(self.shape.near(self.c * x + self.s * y, -self.s * x + self.c * y, reach), 0, (self.c, self.s))


def painted(layers):
    """A 1024 x 1024 RGBA PNG of `layers`, (shape, edge, colour) from the bottom up: each colour painted over what is
    under it where the shape's distance is under the edge, antialiased over a pixel. Worked out in squares of 64 px,
    split in four down to 2 px wherever one isn't one colour throughout."""
    shapes = []
    for shape, _, _ in layers:
        if all(shape is not s for s in shapes):
            shapes.append(shape)
    order = [(next(k for k, s in enumerate(shapes) if s is shape), edge, colour) for shape, edge, colour in layers]
    canvas = bytearray(SIZE * SIZE * 4)

    def colour_of(d):
        r = g = b = a = 0.0
        for k, edge, (cr, cg, cb) in order:
            c = cover(d[k], edge)
            if c > 0.0:
                r, g, b, a = cr * c + r * (1 - c), cg * c + g * (1 - c), cb * c + b * (1 - c), c + a * (1 - c)
        if a <= 0.0:
            return b"\0\0\0\0"
        return bytes([int(round(clamp(v / a, 0.0, 255.0))) for v in (r, g, b)] + [int(round(a * 255))])

    def square(x0, y0, size, given):
        cx, cy = x0 + size / 2 - SIZE / 2, y0 + size / 2 - SIZE / 2
        reach = (size - 1) / 2 * SQRT2 + 1e-6
        local = [s.near(cx, cy, reach) for s in given]
        d = [f(cx, cy) for f in local]
        if all(d[k] - reach >= edge + 0.5 or d[k] + reach <= edge - 0.5 for k, edge, _ in order):
            row = colour_of(d) * size
            for y in range(y0, y0 + size):
                at = (y * SIZE + x0) * 4
                canvas[at:at + size * 4] = row
        elif size > 2:
            half = size // 2
            for dy in (0, half):
                for dx in (0, half):
                    square(x0 + dx, y0 + dy, half, local)
        else:
            for y in range(y0, y0 + size):
                for x in range(x0, x0 + size):
                    px, py = x + 0.5 - SIZE / 2, y + 0.5 - SIZE / 2
                    at = (y * SIZE + x) * 4
                    canvas[at:at + 4] = colour_of([f(px, py) for f in local])

    for y0 in range(0, SIZE, 64):
        for x0 in range(0, SIZE, 64):
            square(x0, y0, 64, shapes)
    raw = bytearray()
    for y in range(SIZE):
        raw.append(0)
        raw.extend(canvas[y * SIZE * 4:(y + 1) * SIZE * 4])
    return png_file(SIZE, SIZE, 6, raw)


def bordered(shape, colour):
    """A sticker's layers: `shape` filled with `colour`, in a white border."""
    return [(shape, BORDER, WHITE), (shape, 0.0, colour)]


def outlined(shape, colour):
    """A sticker's layers: `shape` filled with `colour` inside a navy outline, in a white border."""
    return [(shape, BORDER, WHITE), (shape, 0.0, NAVY), (shape, -OUTLINE, colour)]


def arc(cx, cy, rx, ry, start, end, step=2.0):
    """Points along an ellipse from `start` to `end` degrees (counterclockwise, y up, when `end` is the larger)."""
    n = max(1, int(abs(end - start) / step + 0.5))
    return [(cx + rx * cos((start + (end - start) * k / n) * PI / 180),
             cy + ry * sin((start + (end - start) * k / n) * PI / 180)) for k in range(n + 1)]


# Klipolo's stroke letters: each a width and its lines, in units of the capital height (0 at the baseline, y up).
GLYPHS = {
    "1": (0.36, [[(0.0, 0.78), (0.3, 1.0), (0.3, 0.0)]]),
    "2": (0.6, [arc(0.29, 0.71, 0.27, 0.29, 165, -35) + [(0.02, 0.0), (0.6, 0.0)]]),
    "3": (0.58, [arc(0.28, 0.76, 0.25, 0.23, 155, -90) + arc(0.28, 0.27, 0.3, 0.26, 90, -155)]),
    "4": (0.62, [[(0.46, 0.0), (0.46, 1.0), (0.0, 0.32), (0.62, 0.32)]]),
    "5": (0.6, [[(0.54, 1.0), (0.11, 1.0), (0.07, 0.542)] + arc(0.3, 0.33, 0.3, 0.33, 140, -150)]),
    "?": (0.58, [arc(0.29, 0.72, 0.28, 0.27, 165, -60) + [(0.29, 0.38), (0.29, 0.28)], [(0.29, 0.0)]]),
    "!": (0.0, [[(0.0, 1.0), (0.0, 0.3)], [(0.0, 0.0)]]),
    "B": (0.57, [[(0.3, 0.53), (0.0, 0.53), (0.0, 1.0), (0.28, 1.0)] + arc(0.28, 0.765, 0.24, 0.235, 90, -90),
                 [(0.0, 0.53), (0.0, 0.0), (0.3, 0.0)] + arc(0.3, 0.265, 0.27, 0.265, -90, 90)]),
    "C": (0.6, [arc(0.34, 0.5, 0.34, 0.5, 50, 310)]),
    "E": (0.5, [[(0.5, 1.0), (0.0, 1.0), (0.0, 0.0), (0.5, 0.0)], [(0.0, 0.5), (0.42, 0.5)]]),
    "I": (0.0, [[(0.0, 0.0), (0.0, 1.0)]]),
    "N": (0.62, [[(0.0, 0.0), (0.0, 1.0), (0.62, 0.0), (0.62, 1.0)]]),
    "R": (0.56, [[(0.0, 0.0), (0.0, 1.0), (0.28, 1.0)] + arc(0.28, 0.75, 0.25, 0.25, 90, -90) + [(0.0, 0.5)],
                 [(0.24, 0.5), (0.56, 0.0)]]),
    "S": (0.6, [arc(0.3, 0.75, 0.28, 0.25, 20, 270) + arc(0.3, 0.25, 0.3, 0.25, 90, -160)[1:]]),
    "U": (0.6, [[(0.0, 1.0), (0.0, 0.32)] + arc(0.3, 0.32, 0.3, 0.32, 180, 360) + [(0.6, 1.0)]]),
    "W": (0.84, [[(0.0, 1.0), (0.21, 0.0), (0.42, 0.72), (0.63, 0.0), (0.84, 1.0)]]),
}


def text(word, cx, cy, cap, width):
    """`word` in Klipolo's stroke letters, `cap` px tall in lines `width` px wide, centred on (cx, cy)."""
    gap = 0.22 + width / cap  # between letters, past their round ends
    total = sum(GLYPHS[c][0] for c in word) + gap * (len(word) - 1)
    x, lines = cx - total * cap / 2, []
    for c in word:
        w, glyph = GLYPHS[c]
        lines += [[(x + u * cap, cy + (0.5 - v) * cap) for u, v in line] for line in glyph]
        x += (w + gap) * cap
    return Strokes(lines, width)


def screen(points):
    """Points given y up as the canvas has them, y down."""
    return [(x, -y) for x, y in points]


def arrow_head(tip_x, tip_y, dx, dy, length_, width_):
    """A triangle pointing along (dx, dy) to the tip, `length_` long and `width_` across its base."""
    n = length(dx, dy)
    ux, uy = dx / n, dy / n
    bx, by = tip_x - ux * length_, tip_y - uy * length_
    return [(tip_x, tip_y), (bx + uy * width_ / 2, by - ux * width_ / 2), (bx - uy * width_ / 2, by + ux * width_ / 2)]


def sparkle_star(cx, cy, r):
    """A four-point sparkle: all but the four circles round a square's corners, grown 14 px, then cut to a circle with
    the cut's corners rounded, so each point ends round."""
    corners = [Outside(Circle(cx + sx * r, cy + sy * r, r)) for sx in (-1, 1) for sy in (-1, 1)]
    return Within(Grown(Within(*corners), 14.0), Circle(cx, cy, r + 4.0), k=28.0)


def curved_arrow():
    """A thick blue arrow arcing over from the bottom left to point down at the right."""
    line = screen(arc(-10.0, -120.0, 320.0, 320.0, 200, 32))
    (ax, ay), (bx, by) = line[-2], line[-1]
    ux, uy = (bx - ax) / length(bx - ax, by - ay), (by - ay) / length(bx - ax, by - ay)
    head = Polygon(arrow_head(bx + 120.0 * ux, by + 120.0 * uy, ux, uy, 150.0, 250.0), 16.0)
    return bordered(Union(Strokes([line], 100.0), head), BLUE)


def pointer_arrow():
    """A red arrow pointing down to the left, as at something in the video."""
    shaft = Strokes([[(330.0, -330.0), (-150.0, 150.0)]], 90.0)
    return bordered(Union(shaft, Polygon(arrow_head(-350.0, 350.0, -1.0, 1.0, 270.0, 300.0), 16.0)), RED)


def drawn_circle():
    """A red loop as if drawn round something with a marker: a little more than once round, not quite closing."""
    loop = []
    for k in range(201):
        s = k / 200
        a = (100 + 400 * s) * PI / 180
        loop.append(((390 - 34 * s) * cos(a) + 10 - 22 * s, -(290 - 26 * s) * sin(a) + 14 * s))
    return bordered(Turned(Strokes([loop], 56.0), -8.0), RED)


def underline():
    """A gold marker stroke, slightly curved, to underline words."""
    swash = [(-420 + 840 * s, 70 - 150 * s + 260 * (s - 0.5) * (s - 0.5)) for s in (k / 24 for k in range(25))]
    return bordered(Strokes([swash], 76.0), GOLD)


def x_mark():
    """A red cross."""
    return bordered(Strokes([[(-250.0, -250.0), (250.0, 250.0)], [(250.0, -250.0), (-250.0, 250.0)]], 130.0), RED)


def number(n):
    """A yellow badge with a navy number."""
    return bordered(Circle(0.0, 0.0, 400.0), HILITE) + [(text(n, 0.0, 0.0, 440.0, 96.0), 0.0, NAVY)]


def thought_bubble():
    """A white cloud in a navy outline, with two small puffs trailing from its bottom left."""
    puffs = [(0.0, -150.0, 190.0), (-230.0, -60.0, 160.0), (230.0, -60.0, 165.0), (-150.0, 90.0, 150.0),
             (130.0, 100.0, 160.0), (0.0, -20.0, 200.0), (-250.0, 280.0, 58.0), (-345.0, 385.0, 36.0)]
    return outlined(Union(*[Circle(x, y, r) for x, y, r in puffs]), WHITE)


def shout_bubble():
    """A gold burst of 14 spikes in a navy outline, for a shout or a wow."""
    burst = []
    for k in range(28):
        a = k / 28 * 2 * PI + 0.1
        r = (440.0 if k % 4 == 0 else 405.0) if k % 2 == 0 else 290.0
        burst.append((r * cos(a), 0.82 * r * sin(a)))
    return outlined(Polygon(burst, 8.0), GOLD)


def thumbs_up():
    """A gold fist with its thumb up, in a navy outline, from a blue cuff."""
    hand = Smooth(Box(70.0, 140.0, 190.0, 195.0, 80.0), Taper(-60.0, -10.0, 80.0, -10.0, -360.0, 62.0), 50.0)
    cuff = Box(-250.0, 140.0, 90.0, 225.0, 28.0)
    fingers = Strokes([[(120.0, y), (245.0, y)] for y in (40.0, 135.0, 230.0)], 16.0)
    return ([(Union(hand, cuff), BORDER, WHITE)] + outlined(cuff, BLUE)[1:] + outlined(hand, GOLD)[1:]
            + [(fingers, 0.0, NAVY)])


def fire():
    """An orange flame of three tongues round a gold core."""
    flame = Union(Circle(0.0, 170.0, 260.0), Taper(0.0, 170.0, 240.0, 40.0, -440.0, 10.0),
                  Taper(-60.0, 120.0, 190.0, -270.0, -210.0, 10.0), Taper(60.0, 120.0, 190.0, 260.0, -130.0, 10.0),
                  k=60.0)
    core = Union(Circle(0.0, 240.0, 150.0), Taper(0.0, 240.0, 140.0, 20.0, -110.0, 6.0), k=40.0)
    return bordered(flame, ORANGE) + [(core, 0.0, GOLD)]


def sparkles():
    """Three gold four-point sparkles, one large and two small."""
    stars = Union(sparkle_star(-80.0, 70.0, 330.0), sparkle_star(250.0, -250.0, 160.0),
                  sparkle_star(270.0, 260.0, 110.0))
    return bordered(stars, GOLD)


def lightbulb():
    """A gold bulb in a navy outline on a grey screw base, with five short rays above it."""
    rays = Strokes([[(290.0 * cos(a * PI / 180), -60.0 - 290.0 * sin(a * PI / 180)),
                     (360.0 * cos(a * PI / 180), -60.0 - 360.0 * sin(a * PI / 180))] for a in (10, 50, 90, 130, 170)],
                   46.0)
    glass = Smooth(Circle(0.0, -60.0, 230.0), Taper(0.0, 20.0, 180.0, 0.0, 250.0, 120.0), 40.0)
    base = Box(0.0, 320.0, 125.0, 80.0, 30.0)
    threads = Strokes([[(-110.0, y), (110.0, y)] for y in (295.0, 345.0)], 14.0)
    return (bordered(rays, GOLD) + [(Union(glass, base), BORDER, WHITE)] + outlined(glass, GOLD)[1:]
            + outlined(base, GREY)[1:] + [(threads, 0.0, NAVY)])


def new_tag():
    """A red tag tilted up a little, with NEW in white."""
    tag, word = Turned(Box(0.0, 0.0, 400.0, 170.0, 60.0), -8.0), Turned(text("NEW", 0.0, 0.0, 190.0, 44.0), -8.0)
    return bordered(tag, RED) + [(word, 0.0, WHITE)]


def subscribe():
    """A red pill with SUBSCRIBE in white: plain words, no one's logo."""
    return bordered(Box(0.0, 0.0, 460.0, 130.0, 130.0), RED) + [(text("SUBSCRIBE", 0.0, 0.0, 96.0, 24.0), 0.0, WHITE)]


def cursor_hand():
    """A white pointing hand in a navy outline, as a cursor over a link: the index finger up, the others folded."""
    # The fingers overlap by 40 px, deeper than the outline, so only the creases mark where they meet.
    hand = Union(Strokes([[(-50.0, -380.0), (-50.0, 20.0)]], 120.0),
                 Strokes([[(30.0, -90.0), (30.0, 40.0)], [(110.0, -60.0), (110.0, 60.0)],
                          [(190.0, -30.0), (190.0, 70.0)]], 120.0),
                 Box(60.0, 170.0, 210.0, 170.0, 110.0), Taper(-120.0, 140.0, 65.0, -260.0, -10.0, 50.0), k=24.0)
    creases = Strokes([[(-10.0, -120.0), (-10.0, 60.0)], [(70.0, -95.0), (70.0, 80.0)],
                       [(150.0, -65.0), (150.0, 90.0)]], 14.0)
    return outlined(hand, WHITE) + [(creases, 0.0, NAVY)]


# ------------------------------------------------------------------------------------------------- backgrounds
#
# Pictures sized for a 16:9 frame (1920 x 1080) or a 9:16 one (1080 x 1920), as opaque RGB PNGs. Each is worked out
# on a grid of points 8 px apart (two more past every edge), drawn up to full size by a cubic B-spline (smooth, and
# never past the points' own colours), and dithered so its slow gradients don't band.

STEP = 8
BAYER = [[0, 32, 8, 40, 2, 34, 10, 42], [48, 16, 56, 24, 50, 18, 58, 26], [12, 44, 4, 36, 14, 46, 6, 38],
         [60, 28, 52, 20, 62, 30, 54, 22], [3, 35, 11, 43, 1, 33, 9, 41], [51, 19, 59, 27, 49, 17, 57, 25],
         [15, 47, 7, 39, 13, 45, 5, 37], [63, 31, 55, 23, 61, 29, 53, 21]]


def spline_weights():
    """For each of a cell's 8 pixels: which grid point its 4 start at (past the cell's first), and their weights."""
    out = []
    for p in range(STEP):
        f = (p - 3.5) / STEP
        start, f = (0, f + 1) if f < 0 else (1, f)
        g, cube = 1 - f, f * f * f
        weights = (g * g * g / 6, (3 * cube - 6 * f * f + 4) / 6, (-3 * cube + 3 * f * f + 3 * f + 1) / 6, cube / 6)
        out.append((start, weights))
    return out


def full_size(paint, width, height, extra):
    """Rows of `width` x `height` RGB bytes: `paint(u, v)` (u and v from 0 to 1 across and down the frame) on the grid,
    drawn up to full size. `extra` are rows of a number for each byte, added before rounding down (an ordered dither, or
    a half and a grain), used in turn."""
    weights, cols = spline_weights(), width // STEP

    def point(j, size):
        return (STEP * (j - 2) + 4) / size

    across = []
    for i in range(height // STEP + 4):
        row = [paint(point(j, width), point(i, height)) for j in range(cols + 4)]
        channels = []
        for c in range(3):
            line = [clamp(px[c], 0.0, 255.0) for px in row]
            full = [0.0] * width
            for p, (s, (w0, w1, w2, w3)) in enumerate(weights):
                full[p::STEP] = [a * w0 + b * w1 + d * w2 + e * w3
                                 for a, b, d, e in zip(line[s:], line[s + 1:], line[s + 2:], line[s + 3:])][:cols]
            channels.append(full)
        across.append(channels)
    out = []
    for i in range(height // STEP):
        for p, (s, (w0, w1, w2, w3)) in enumerate(weights):
            a, b, d, e = across[i + s:i + s + 4]
            row = [0.0] * (3 * width)
            for c in range(3):
                row[c::3] = [q * w0 + r * w1 + t * w2 + u * w3 for q, r, t, u in zip(a[c], b[c], d[c], e[c])]
            add = extra[len(out) % len(extra)]
            out.append(bytes([min(255, max(0, int(v + k))) for v, k in zip(row, add)]))
    return out


def png_rgb(rows, width, height):
    """An RGB PNG of `rows`, each filtered as the difference from the pixel to its left."""
    raw = bytearray()
    for row in rows:
        raw.append(1)
        raw.extend(row[:3])
        raw.extend(bytes([(a - b) & 255 for a, b in zip(row[3:], row)]))
    return png_file(width, height, 2, raw)


def dither(width):
    """An 8 x 8 ordered dither: for each row, a number in (0, 1) for each byte, the same for a pixel's three."""
    return [[(BAYER[y][x % 8] + 0.5) / 64 for x in range(width) for _ in range(3)] for y in range(8)]


def mix(a, b, t):
    return [x + (y - x) * t for x, y in zip(a, b)]


def blobs(base, spots, u, v, width, height):
    """`base` with soft round `spots` (u, v, radius as a share of the frame's shorter side, colour, strength) laid
    over it in turn, each fading out from its middle like a blur."""
    short = min(width, height)
    colour = list(base)
    for su, sv, radius, spot, strength in spots:
        dx, dy = (u - su) * width / short, (v - sv) * height / short
        colour = mix(colour, spot, strength * exp(-(dx * dx + dy * dy) / (radius * radius)))
    return colour


def sunset(width, height):
    """A warm gradient from the top left to the bottom right, gold through coral to violet, with a soft glow of sun."""
    stops = [(0.0, hex_rgb("#FFB547")), (0.5, hex_rgb("#FF5C7A")), (1.0, hex_rgb("#6B4BFF"))]

    def paint(u, v):
        t = clamp((u * width * width + v * height * height) / (width * width + height * height), 0.0, 1.0)
        (t0, a), (t1, b) = (stops[0], stops[1]) if t < 0.5 else (stops[1], stops[2])
        s = (t - t0) / (t1 - t0)
        return blobs(mix(a, b, s * s * (3 - 2 * s)), [(0.8, 0.16, 0.42, hex_rgb("#FFE6A8"), 0.5)], u, v, width, height)

    return png_rgb(full_size(paint, width, height, dither(width)), width, height)


def soft_shapes(width, height):
    """Pastel blurs of peach, lilac, mint, sky and apricot on cream."""
    spots = [(0.12, 0.18, 0.55, hex_rgb("#FFB49A"), 0.85), (0.88, 0.22, 0.5, hex_rgb("#C8B6FF"), 0.85),
             (0.25, 0.92, 0.5, hex_rgb("#A6E8D2"), 0.85), (0.8, 0.85, 0.55, hex_rgb("#A9D4FF"), 0.85),
             (0.55, 0.5, 0.35, hex_rgb("#FFD9A0"), 0.6)]

    def paint(u, v):
        return blobs(hex_rgb("#F8F2EA"), spots, u, v, width, height)

    return png_rgb(full_size(paint, width, height, dither(width)), width, height)


def night_glow(width, height):
    """Deep navy with blurred glows of violet, teal and pink, for white words to stand out on."""
    spots = [(0.18, 0.22, 0.6, hex_rgb("#6C4DFF"), 0.8), (0.88, 0.78, 0.65, hex_rgb("#18B6C8"), 0.55),
             (0.72, 0.02, 0.4, hex_rgb("#FF4D9D"), 0.5)]

    def paint(u, v):
        return blobs(mix(hex_rgb("#0B1030"), hex_rgb("#151C48"), clamp(v, 0.0, 1.0)), spots, u, v, width, height)

    return png_rgb(full_size(paint, width, height, dither(width)), width, height)


def scrambled(n):
    """`n` scrambled into a number in [-1, 1) (a 32-bit integer hash): the same on every machine."""
    n &= 0xFFFFFFFF
    n ^= n >> 16
    n = (n * 0x7FEB352D) & 0xFFFFFFFF
    n ^= n >> 15
    n = (n * 0x846CA68B) & 0xFFFFFFFF
    n ^= n >> 16
    return n / 2147483648.0 - 1.0


def smooth_noise(x, y, seed):
    """Noise that changes smoothly: a scrambled number at each whole (x, y), eased between them."""
    i, j = math.floor(x), math.floor(y)
    fx, fy = x - i, y - j
    fx, fy = fx * fx * (3 - 2 * fx), fy * fy * (3 - 2 * fy)

    def at(a, b):
        return scrambled(a * 73856093 ^ b * 19349663 ^ seed * 83492791)

    top = at(i, j) + (at(i + 1, j) - at(i, j)) * fx
    bottom = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * fx
    return top + (bottom - top) * fy


def paper(width, height):
    """Warm off-white paper: faint blotches, edges a touch darker, and a fine grain. The grain is whole steps of
    brightness from a 256 px tile, so most rows repeat every 256 px and the file stays small."""
    noise = Noise(211)
    raw = [[noise() for _ in range(257)] for _ in range(257)]
    grain = [[round(4.0 * (raw[y][x] + raw[y][x + 1] + raw[y + 1][x] + raw[y + 1][x + 1]) / 4) for x in range(256)]
             for y in range(256)]
    extra = [[0.5 + grain[y][x % 256] for x in range(width) for _ in range(3)] for y in range(256)]
    base = hex_rgb("#F3ECDE")

    def paint(u, v):
        x, y = u * width, v * height
        blotch = 0.7 * smooth_noise(x / 160, y / 160, 1) + 0.3 * smooth_noise(x / 55, y / 55, 2)
        dx, dy = (u - 0.5) * 2, (v - 0.5) * 2
        shade = 1.0 - 0.03 * (dx * dx + dy * dy) + 0.012 * blotch
        return [c * shade for c in base]

    return png_rgb(full_size(paint, width, height, extra), width, height)


BACKGROUNDS = [
    ("sunset", "Sunset gradient", ["background", "gradient", "sunset", "warm"], sunset),
    ("soft", "Soft shapes", ["background", "pastel", "soft", "blur"], soft_shapes),
    ("night", "Night glow", ["background", "dark", "glow", "night"], night_glow),
    ("paper", "Paper", ["background", "paper", "texture", "light"], paper),
]


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
    ("impact", "Impact", ["impact", "boom", "hit", "cinematic", "transition"], impact, 0.4),
    ("bass-drop", "Bass drop", ["bass drop", "drop", "bass", "boom", "transition"], bass_drop, 0.35),
    ("long-riser", "Long riser", ["riser", "build up", "tension", "suspense", "intro"], long_riser, 0.5),
    ("swipe-left", "Swipe left", ["swipe", "left", "slide", "transition", "quick"], lambda: swipe(False, 139), 0.5),
    ("swipe-right", "Swipe right", ["swipe", "right", "slide", "transition", "quick"], lambda: swipe(True, 137), 0.5),
    ("glitch", "Glitch", ["glitch", "digital", "error", "tech", "transition"], glitch, 0.35),
    ("record-scratch", "Record scratch", ["record scratch", "scratch", "stop", "rewind", "funny"],
     record_scratch, 0.6),
    ("shutter", "Camera shutter", ["camera", "shutter", "photo", "snap", "picture"], shutter, 0.68),
    ("typing", "Keyboard typing", ["typing", "keyboard", "keys", "text", "computer"], typing, 0.68),
    ("notification", "Notification", ["notification", "message", "alert", "phone", "ping"], notification, 0.5),
    ("success", "Success", ["success", "win", "level up", "correct", "game"], success, 0.5),
    ("error", "Error buzz", ["error", "wrong", "buzz", "fail", "no"], error_buzz, 0.33),
    ("pop-high", "High pop", ["pop", "high", "appear", "cute", "short"], pop_high, 0.55),
    ("bubbles", "Bubble pops", ["bubbles", "pop", "popping", "appear", "fun"], bubbles, 0.55),
    ("boing", "Cartoon boing", ["boing", "bounce", "spring", "cartoon", "funny"], boing, 0.42),
    ("drum-hit", "Drum hit", ["drum", "hit", "punch", "snare", "kick"], drum_hit, 0.45),
    ("tick-tock", "Tick tock", ["clock", "tick tock", "timer", "countdown", "waiting"], tick_tock, 0.55),
]

MUSIC = [
    ("soft-ambient", "Soft ambient", ["calm", "ambient", "soft", "chill", "loop"], soft_ambient),
    ("upbeat-loop", "Upbeat loop", ["upbeat", "happy", "energetic", "pop", "loop"], upbeat_loop),
    ("chill-beat", "Chill beat", ["lofi", "chill", "study", "relaxed", "loop"], chill_beat),
    ("energetic-loop", "Energetic loop", ["energetic", "hype", "fast", "sport", "intense", "loop"], energetic_loop),
]

STICKERS = [
    ("heart", "Heart", ["heart", "love", "like", "pink"], heart, "#FF4D6D"),
    ("star", "Star", ["star", "favourite", "best", "gold"], star, "#FFC21A"),
    ("arrow", "Arrow", ["arrow", "point", "look", "right", "blue"], arrow, "#4C8DFF"),
    ("circle", "Circle", ["circle", "ring", "highlight", "look here", "orange"], ring, "#FF6A3D"),
    ("check", "Check", ["check", "tick", "done", "yes", "green"], check, "#22C55E"),
]

DRAWN = [  # drawn as layers (`painted`), after the speech bubble
    ("arrow-curved", "Curved arrow", ["arrow", "curved", "point", "look", "blue"], curved_arrow),
    ("arrow-pointer", "Pointer arrow", ["arrow", "pointer", "point", "look here", "red"], pointer_arrow),
    ("circle-drawn", "Drawn circle", ["circle", "highlight", "look here", "marker", "red"], drawn_circle),
    ("underline", "Underline", ["underline", "highlight", "marker", "emphasis", "gold"], underline),
    ("x-mark", "X mark", ["x", "cross", "wrong", "no", "red"], x_mark),
    ("question", "Question mark", ["question", "question mark", "why", "quiz", "purple"],
     lambda: bordered(text("?", 0.0, 0.0, 760.0, 130.0), PURPLE)),
    ("exclamation", "Exclamation mark", ["exclamation", "exclamation mark", "wow", "alert", "important", "red"],
     lambda: bordered(text("!", 0.0, 0.0, 760.0, 140.0), RED)),
    ("number-1", "Number 1", ["number", "1", "one", "step", "first"], lambda: number("1")),
    ("number-2", "Number 2", ["number", "2", "two", "step", "second"], lambda: number("2")),
    ("number-3", "Number 3", ["number", "3", "three", "step", "third"], lambda: number("3")),
    ("number-4", "Number 4", ["number", "4", "four", "step", "fourth"], lambda: number("4")),
    ("number-5", "Number 5", ["number", "5", "five", "step", "fifth"], lambda: number("5")),
    ("thought-bubble", "Thought bubble", ["thought", "think", "idea", "cloud", "white"], thought_bubble),
    ("shout-bubble", "Shout bubble", ["shout", "burst", "wow", "loud", "comic"], shout_bubble),
    ("thumbs-up", "Thumbs up", ["thumbs up", "like", "yes", "good", "approve"], thumbs_up),
    ("fire", "Fire", ["fire", "flame", "hot", "trending", "lit"], fire),
    ("sparkles", "Sparkles", ["sparkles", "shine", "magic", "new", "gold"], sparkles),
    ("lightbulb", "Lightbulb", ["lightbulb", "idea", "tip", "think", "smart"], lightbulb),
    ("new-tag", "New tag", ["new", "tag", "label", "launch", "red"], new_tag),
    ("subscribe", "Subscribe button", ["subscribe", "button", "follow", "channel", "call to action"], subscribe),
    ("cursor-hand", "Cursor hand", ["cursor", "hand", "click", "pointer", "tap"], cursor_hand),
]

CC0 = ("To the extent possible under law, Klipolo has waived all copyright and related or neighbouring rights to these "
       "files.\nThey are dedicated to the public domain under CC0 1.0 Universal:\n"
       "https://creativecommons.org/publicdomain/zero/1.0/\n")


SHAPES = ((1920, 1080, "16:9", "wide"), (1080, 1920, "9:16", "vertical"))
SPEECH_TAGS = ["speech", "talk", "say", "quote", "white"]


def made(key):
    """One item's file, and its length or size. Each item is made on its own, so several are made at once."""
    kind, k = key
    if kind == "sound":
        samples = normalised(SOUNDS[k][3](), SOUNDS[k][4])
        return wav_bytes(samples, SR), dict(duration_ms=round(len(samples) * 1000 / SR))
    if kind == "music":
        samples = MUSIC[k][3]()
        return wav_bytes(samples, MUSIC_RATE), dict(duration_ms=round(len(samples) * 1000 / MUSIC_RATE))
    if kind == "sticker":
        return sticker(STICKERS[k][3], STICKERS[k][4]), dict(width=SIZE, height=SIZE)
    if kind == "speech":
        return png_bytes(layered(bubble, [(BORDER, WHITE), (0.0, NAVY), (-18.0, WHITE)]), 470), dict(width=SIZE,
                                                                                                    height=SIZE)
    if kind == "drawn":
        return painted(DRAWN[k][3]()), dict(width=SIZE, height=SIZE)
    design, (width, height) = k
    return BACKGROUNDS[design][3](width, height), dict(width=width, height=height)


def build(workers=None):
    """Every file of the pack, by path, and the manifest's items. The items are made by `workers` processes at once
    (default: one a CPU, at most 8), the longest first; the bytes are the same however many."""
    listed = [(id_, name, "sound", f"sounds/{id_}.wav", tags, ("sound", k))
              for k, (id_, name, tags, _, _) in enumerate(SOUNDS)]
    listed += [(id_, name, "music", f"music/{id_}.wav", tags, ("music", k))
               for k, (id_, name, tags, _) in enumerate(MUSIC)]
    listed += [(id_, name, "sticker", f"stickers/{id_}.png", tags, ("sticker", k))
               for k, (id_, name, tags, _, _) in enumerate(STICKERS)]
    listed.append(("bubble", "Speech bubble", "sticker", "stickers/bubble.png", SPEECH_TAGS, ("speech", 0)))
    listed += [(id_, name, "sticker", f"stickers/{id_}.png", tags, ("drawn", k))
               for k, (id_, name, tags, _) in enumerate(DRAWN)]
    for k, (id_, name, tags, _) in enumerate(BACKGROUNDS):  # listed as stickers: the pack's pictures, on the Images tab
        for width, height, ratio, shape in SHAPES:
            key = f"{id_}-{ratio.replace(':', 'x')}"
            listed.append((key, f"{name} {ratio}", "sticker", f"backgrounds/{key}.png", tags + [ratio, shape],
                           ("background", (k, (width, height)))))

    keys = sorted((entry[-1] for entry in listed), key=lambda key: key[0] not in ("music", "background"))
    with concurrent.futures.ProcessPoolExecutor(workers or min(os.cpu_count() or 1, 8)) as pool:
        made_by = {key: pool.submit(made, key) for key in keys}
        results = [made_by[entry[-1]].result() for entry in listed]

    files, items = {}, []
    for (id_, name, category, path, tags, _), (data, size) in zip(listed, results):
        files[path] = data
        items.append(dict(id=id_, name=name, category=category, file=path, **size, tags=tags,
                          sha256=hashlib.sha256(data).hexdigest(), license="CC0-1.0"))
    manifest = dict(version=1, id="klipolo-pack", name="Klipolo pack", license="CC0-1.0", creator="Klipolo",
                    items=items)
    files["manifest.json"] = (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode()
    stickers = len(STICKERS) + 1 + len(DRAWN)  # with the speech bubble
    holds = (f"{len(SOUNDS)} sound effects, {len(MUSIC)} music beds, {stickers} stickers and "
             f"{len(BACKGROUNDS) * len(SHAPES)} backgrounds")
    files["LICENSE.txt"] = (f"Klipolo pack: {holds},\nmade from code by scripts/gen-library.py. No recordings, "
                            f"samples or outside media were used.\n\n{CC0}").encode()
    return files


def main():
    parser = argparse.ArgumentParser(description="Write Klipolo's bundled CC0 pack.")
    default = pathlib.Path(__file__).resolve().parents[1] / "assets" / "library"
    parser.add_argument("--out", type=pathlib.Path, default=default, help=f"where to write it (default {default})")
    parser.add_argument("--workers", type=int, help="processes making items at once (default: one a CPU, at most 8)")
    args = parser.parse_args()
    files = build(args.workers)
    for path, data in files.items():
        target = args.out / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    print(f"Wrote {len(files)} files, {sum(len(d) for d in files.values())} bytes, into {args.out}")


if __name__ == "__main__":
    main()
