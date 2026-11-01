#!/usr/bin/env python3
"""Measures each sound effect of Klipolo's pack, since no one listens on the build machine: its length, peak, overall
and loudest 20 ms levels, DC offset, clipped samples, and the silence before and after it. Prints one line a sound and
fails on any outside the bounds below, or much louder or quieter than the pack's other sounds.

Python 3 standard library only. Usage: python3 scripts/check-library-sounds.py [DIR] (default assets/library).
"""

import json
import math
import pathlib
import struct
import sys
import wave

FULL = 32767
LONGEST = 3.0  # seconds; a sound at least this long is music in the library
SHORTEST = 0.04
PEAK_DB = (-10.0, -3.0)  # headroom under 0 dBFS, and loud enough to hear
LOUD_DB = (-26.0, -8.0)  # the loudest 20 ms
FROM_MEDIAN_DB = 9.0  # how far a sound's loudest 20 ms may sit from the pack's median
MAX_DC = 0.002  # of full scale, about -54 dBFS
MAX_LEAD = 0.1  # seconds before the sound reaches a thousandth of its peak (a riser starts from nothing)
MAX_TAIL = 0.05  # seconds after it last reaches a thousandth of its peak
MAX_END = 64  # the first and last samples: silence, so placing it never clicks


def db(x):
    return 20 * math.log10(x) if x > 0 else float("-inf")


def measure(path):
    with wave.open(str(path), "rb") as w:
        rate, n = w.getframerate(), w.getnframes()
        s = struct.unpack(f"<{n}h", w.readframes(n))
    peak = max(abs(v) for v in s)
    window, hop = int(0.02 * rate), int(0.005 * rate)
    squares = [0]
    for v in s:
        squares.append(squares[-1] + v * v)
    loud = max(squares[min(i + window, n)] - squares[i] for i in range(0, max(1, n - window + 1), hop))
    loud = math.sqrt(loud / min(window, n))
    first = next(i for i, v in enumerate(s) if abs(v) * 1000 >= peak)
    last = max(i for i, v in enumerate(s) if abs(v) * 1000 >= peak)
    return dict(
        seconds=n / rate,
        peak=db(peak / FULL),
        rms=db(math.sqrt(squares[-1] / n) / FULL),
        loud=db(loud / FULL),
        dc=sum(s) / n / FULL,
        clipped=sum(abs(v) >= FULL for v in s),
        lead=first / rate,
        tail=(n - 1 - last) / rate,
        ends=max(abs(s[0]), abs(s[-1])),
    )


def problems(m, median):
    out = []
    if not SHORTEST <= m["seconds"] < LONGEST:
        out.append(f"{m['seconds']:.2f} s long")
    if not PEAK_DB[0] <= m["peak"] <= PEAK_DB[1]:
        out.append(f"peak {m['peak']:.1f} dBFS")
    if m["clipped"]:
        out.append(f"{m['clipped']} clipped samples")
    if not LOUD_DB[0] <= m["loud"] <= LOUD_DB[1]:
        out.append(f"loudest 20 ms {m['loud']:.1f} dBFS")
    if abs(m["loud"] - median) > FROM_MEDIAN_DB:
        out.append(f"{m['loud'] - median:+.1f} dB from the pack's median")
    if abs(m["dc"]) > MAX_DC:
        out.append(f"DC offset {m['dc']:+.4f}")
    if m["lead"] > MAX_LEAD:
        out.append(f"{m['lead'] * 1000:.0f} ms of silence first")
    if m["tail"] > MAX_TAIL:
        out.append(f"{m['tail'] * 1000:.0f} ms of silence after")
    if m["ends"] > MAX_END:
        out.append(f"starts or ends at {m['ends']}, not silence")
    return out


def main():
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).resolve().parents[1] / "assets"
                        / "library")
    items = [i for i in json.loads((root / "manifest.json").read_text())["items"] if i["category"] == "sound"]
    measured = [(i, measure(root / i["file"])) for i in items]
    louds = sorted(m["loud"] for _, m in measured)
    median = louds[len(louds) // 2]
    print(f"{'sound':<16} {'length':>7} {'peak':>7} {'rms':>7} {'loudest':>8} {'dc':>8} {'lead':>6} {'tail':>6}")
    failed = 0
    for i, m in measured:
        found = problems(m, median)
        failed += bool(found)
        print(f"{i['id']:<16} {m['seconds']:>6.2f}s {m['peak']:>7.1f} {m['rms']:>7.1f} {m['loud']:>8.1f} "
              f"{m['dc']:>+8.4f} {m['lead'] * 1000:>4.0f}ms {m['tail'] * 1000:>4.0f}ms"
              + (f"  FAIL: {'; '.join(found)}" if found else ""))
    print(f"{len(measured)} sounds, median loudest 20 ms {median:.1f} dBFS, {failed} outside the bounds")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
