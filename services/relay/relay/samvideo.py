"""The video SAM sees through the relay, made by the relay.

SAM bills every frame of the video it is sent, so the relay never counts frames
in the person's file: it decodes the upload with ffmpeg, in a subprocess, and
writes its own small MP4 (one video track, 2 frames a second, its long side at
most 640, no sound). It knows that file's frames because it wrote them: the
encoder's own count, checked against ffprobe's count of the packets in the
file. Nothing here runs on the event loop but the waits.

Limits: the caller runs at most two at once (`app.state.sam_videos`); each
step shares one timeout; the upload's size cap is the caller's; a video over
`MAX_SECONDS` is refused, and the encode stops there whatever the header says.
"""

import asyncio
import json
import logging
import math
from pathlib import Path

from relay.errors import RelayError

logger = logging.getLogger(__name__)

FFMPEG = "ffmpeg"
FFPROBE = "ffprobe"
# The most video SAM takes from one upload, and the frames a second it gets.
MAX_SECONDS = 600
FPS = 2
LONG_SIDE = 640

UNREADABLE = "That video can't be read. Try another part of it."
TOO_LONG = "Video tracking works on at most ten minutes at a time. Pick a shorter part."
TOO_SLOW = "Preparing that video took too long. Pick a shorter part, then try again."


async def make(src: Path, out: Path, timeout_s: float) -> int:
    """Writes SAM's copy of `src` to `out` and gives its frames. Refuses a video
    that doesn't decode (400), one over `MAX_SECONDS` (413), or one that takes
    longer than `timeout_s` in all (504)."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout_s

    def left() -> float:
        return max(deadline - loop.time(), 0.0)

    seconds = await _duration(src, left())
    if seconds is not None and seconds > MAX_SECONDS:
        raise RelayError(413, "too_large", TOO_LONG)
    written = await _encode(src, out, left())
    counted = await _packets(out, left())
    if counted <= 0 or counted != written:
        logger.warning("SAM copy: the encoder wrote %s frames, the file holds %s", written, counted)
        raise RelayError(400, "invalid_request", UNREADABLE)
    return counted


async def _duration(src: Path, timeout_s: float) -> float | None:
    """The container's duration in seconds; None when it doesn't say (the encode
    is capped anyway)."""
    out = await _run(
        [FFPROBE, "-v", "error", "-show_entries", "format=duration", "-of", "json", str(src)], timeout_s
    )
    try:
        value = float(json.loads(out)["format"]["duration"])
    except (ValueError, KeyError, TypeError):
        return None
    return value if math.isfinite(value) and value >= 0 else None


async def _encode(src: Path, out: Path, timeout_s: float) -> int:
    """The first video stream only, `FPS` frames a second, its long side at most
    `LONG_SIDE`, no sound, at most `MAX_SECONDS`. Gives the frames it wrote."""
    scale = (
        f"fps={FPS},scale=w='min({LONG_SIDE},iw)':h='min({LONG_SIDE},ih)'"
        ":force_original_aspect_ratio=decrease:force_divisible_by=2"
    )
    progress = await _run(
        [FFMPEG, "-nostdin", "-v", "error", "-i", str(src), "-map", "0:v:0", "-an", "-sn", "-dn",
         "-map_metadata", "-1", "-t", str(MAX_SECONDS), "-vf", scale, "-c:v", "libx264", "-preset", "veryfast",
         "-crf", "30", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-progress", "pipe:1", "-y", str(out)],
        timeout_s,
    )
    frames = [line.split("=", 1)[1] for line in progress.splitlines() if line.startswith("frame=")]
    try:
        return int(frames[-1])
    except (IndexError, ValueError):
        raise RelayError(400, "invalid_request", UNREADABLE) from None


async def _packets(path: Path, timeout_s: float) -> int:
    """The video packets in the relay's own copy, as ffprobe counts them; it
    must hold exactly one stream."""
    out = await _run(
        [FFPROBE, "-v", "error", "-count_packets", "-show_entries", "stream=codec_type,nb_read_packets",
         "-of", "json", str(path)],
        timeout_s,
    )
    try:
        streams = json.loads(out)["streams"]
        [stream] = streams
        if stream["codec_type"] != "video":
            raise ValueError("not video")
        return int(stream["nb_read_packets"])
    except (ValueError, KeyError, TypeError):
        raise RelayError(400, "invalid_request", UNREADABLE) from None


async def _run(args: list[str], timeout_s: float) -> str:
    """`args`' standard output; 504 once `timeout_s` passes (the process is killed), 400 when it fails."""
    if timeout_s <= 0:
        raise RelayError(504, "timeout", TOO_SLOW)
    process = await asyncio.create_subprocess_exec(
        *args, stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
    )
    try:
        out, err = await asyncio.wait_for(process.communicate(), timeout_s)
    except TimeoutError:
        process.kill()
        await process.wait()
        raise RelayError(504, "timeout", TOO_SLOW) from None
    if process.returncode != 0:
        logger.info("SAM copy: %s failed: %s", args[0], err.decode(errors="replace")[-300:])
        raise RelayError(400, "invalid_request", UNREADABLE)
    return out.decode(errors="replace")
