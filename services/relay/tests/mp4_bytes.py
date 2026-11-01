"""Small MP4s for tests: just the boxes a frame count reads, and an mdat."""

import struct


def box(kind: bytes, payload: bytes, *, large: bool = False) -> bytes:
    if large:
        return struct.pack(">I", 1) + kind + struct.pack(">Q", 16 + len(payload)) + payload
    return struct.pack(">I", 8 + len(payload)) + kind + payload


def full_box(kind: bytes, payload: bytes) -> bytes:
    """A box with a version and flags."""
    return box(kind, b"\0\0\0\0" + payload)


def track(handler: bytes, samples: int) -> bytes:
    hdlr = full_box(b"hdlr", b"\0\0\0\0" + handler + b"\0" * 12 + b"name\0")
    stsz = full_box(b"stsz", struct.pack(">II", 0, samples) + b"\0\0\0\x10" * min(samples, 4))
    stbl = box(b"stbl", box(b"stsd", b"\0" * 8) + stsz)
    minf = box(b"minf", box(b"vmhd", b"\0" * 12) + stbl)
    mdia = box(b"mdia", full_box(b"mdhd", b"\0" * 20) + hdlr + minf)
    return box(b"trak", full_box(b"tkhd", b"\0" * 80) + mdia)


def mp4(frames: int, *, moov_first: bool = True, sound: int = 0, large_mdat: bool = False) -> bytes:
    """An MP4 whose video track has `frames` samples (and a sound track of `sound`, before it when given)."""
    ftyp = box(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41")
    tracks = (track(b"soun", sound) if sound else b"") + track(b"vide", frames)
    moov = box(b"moov", full_box(b"mvhd", b"\0" * 96) + tracks)
    mdat = box(b"mdat", b"\x55" * 64, large=large_mdat)
    return ftyp + (moov + mdat if moov_first else mdat + moov)
