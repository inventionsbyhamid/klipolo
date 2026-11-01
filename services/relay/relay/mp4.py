"""A video's frame count, read from its MP4 header as the upload passes.

SAM bills every frame of the video it is sent, so the relay must know a
video's frames before it forwards a SAM call on it (`proxy`). Klipolo's small
copies put their index first (`+faststart`): the `moov` box, which holds the
video track's sample count (`stsz` or `stz2`), comes right after `ftyp`, near
the start of the upload. `FrameCounter` keeps the first `HEAD_LIMIT` bytes of
an upload (its multipart body) and reads the count there; a video whose index
comes after its picture, or isn't in those bytes, is not counted.
"""

import struct

# The most of an upload kept to find its index: a 10 minute copy at 2 frames a
# second has an index of a few tens of KiB.
HEAD_LIMIT = 4 * 1024 * 1024

_NEED_MORE = object()


def video_frames(data: bytes) -> int | None:
    """The video track's frames in the MP4 that starts somewhere in `data`
    (its `ftyp` box), or None when there is no MP4, its index comes after its
    picture, or `data` stops before the index ends."""
    found = _read(data)
    return None if found is _NEED_MORE else found


def _read(data: bytes | bytearray) -> object:
    at = data.find(b"ftyp") - 4
    if at < 0:
        # Not here yet: a multipart body has its part headers (and maybe other parts) first.
        return _NEED_MORE
    while at + 8 <= len(data):
        size, kind, header = _box_header(data, at)
        if size is None:
            return _NEED_MORE
        if kind == b"moov":
            if size == 0 or at + size > len(data):
                return _NEED_MORE
            return _moov_frames(data[at + header : at + size])
        if kind == b"mdat" or size == 0:
            # The picture before the index: not counted as it passes.
            return None
        at += size
    return _NEED_MORE


def _box_header(data: bytes | bytearray, at: int) -> tuple[int | None, bytes, int]:
    """A box's size (0: to the end), type and header length at `at`."""
    (size,) = struct.unpack_from(">I", data, at)
    kind = bytes(data[at + 4 : at + 8])
    if size == 1:
        if at + 16 > len(data):
            return None, kind, 16
        (size,) = struct.unpack_from(">Q", data, at + 8)
        return size, kind, 16
    if 0 < size < 8:
        return None, kind, 8
    return size, kind, 8


def _children(data: bytes):
    """Each box in `data`: (type, payload)."""
    at = 0
    while at + 8 <= len(data):
        size, kind, header = _box_header(data, at)
        if size is None:
            return
        end = len(data) if size == 0 else at + size
        if end > len(data):
            return
        yield kind, data[at + header : end]
        at = end


def _child(data: bytes, kind: bytes) -> bytes | None:
    return next((payload for k, payload in _children(data) if k == kind), None)


def _moov_frames(moov: bytes) -> int | None:
    """The first video track's sample count."""
    for kind, trak in _children(moov):
        if kind != b"trak":
            continue
        mdia = _child(trak, b"mdia")
        hdlr = mdia and _child(mdia, b"hdlr")
        # Version and flags, pre_defined, then the handler type.
        if not hdlr or hdlr[8:12] != b"vide":
            continue
        minf = _child(mdia, b"minf")
        stbl = minf and _child(minf, b"stbl")
        sizes = stbl and (_child(stbl, b"stsz") or _child(stbl, b"stz2"))
        if not sizes or len(sizes) < 12:
            return None
        # Version and flags, then the sample size (stz2: reserved and field size), then the count.
        (count,) = struct.unpack_from(">I", sizes, 8)
        return count
    return None


class FrameCounter:
    """Reads an upload's frame count from its first bytes as they pass
    (`feed`); `frames` is the count once found, else None."""

    def __init__(self) -> None:
        self._head = bytearray()
        self._done = False
        self.frames: int | None = None

    def feed(self, chunk: bytes) -> None:
        if self._done:
            return
        room = HEAD_LIMIT - len(self._head)
        self._head.extend(chunk[:room])
        found = _read(self._head)
        if found is not _NEED_MORE:
            self.frames = found if isinstance(found, int) else None
            self._done = True
        elif len(self._head) >= HEAD_LIMIT:
            self._done = True
