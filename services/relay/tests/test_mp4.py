from relay.mp4 import FrameCounter, video_frames

from tests.mp4_bytes import mp4


def test_frames_are_the_video_tracks_samples_in_a_faststart_mp4():
    assert video_frames(mp4(120)) == 120
    # The sound track's samples don't count, wherever it is.
    assert video_frames(mp4(15_000, sound=900)) == 15_000
    assert video_frames(mp4(7, large_mdat=True)) == 7


def test_a_moov_after_the_mdat_or_no_mp4_counts_nothing():
    assert video_frames(mp4(120, moov_first=False)) is None
    assert video_frames(b"not a video at all") is None
    assert video_frames(b"") is None


def test_the_counter_reads_an_upload_as_it_passes_inside_its_multipart_body():
    body = b"--b\r\nContent-Disposition: form-data; name=\"purpose\"\r\n\r\nuser_data\r\n--b\r\n"
    body += b"Content-Disposition: form-data; name=\"file\"; filename=\"copy.mp4\"\r\n\r\n" + mp4(321) + b"\r\n--b--\r\n"
    counter = FrameCounter()
    for start in range(0, len(body), 7):
        counter.feed(body[start : start + 7])
    assert counter.frames == 321


def test_the_counter_gives_up_past_its_head_limit(monkeypatch):
    import relay.mp4

    monkeypatch.setattr(relay.mp4, "HEAD_LIMIT", 64)
    counter = FrameCounter()
    counter.feed(b"\0" * 200 + mp4(5))
    assert counter.frames is None
