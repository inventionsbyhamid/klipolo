"""POST /v1/align with a fake model: auth, daily minutes, upload limits,
temp files and concurrency. No torch needed."""

import asyncio
import io
import json
import os
import struct
import tempfile
import threading
import time
import uuid
import wave
from contextlib import asynccontextmanager
from datetime import UTC, datetime, timedelta

import anyio
import pytest
from fastapi.testclient import TestClient

from relay import db
from relay.align import FAILED, NO_TEXT, TOO_SHORT, emission_frames
from relay.app import create_app
from relay.config import load_settings
from relay.devices import hash_token

DEVICE = "0f8fad5b-d9cb-469f-a165-70867728950e"
OTHER_DEVICE = "7c9e6679-7425-40de-944b-e07fc1f90ae7"
DASHES = ("–", "—")


def make_wav(seconds=2.0, rate=16000, channels=1, width=2):
    buf = io.BytesIO()
    with wave.open(buf, "wb") as out:
        out.setnchannels(channels)
        out.setsampwidth(width)
        out.setframerate(rate)
        out.writeframes(b"\x01\x00" * (int(seconds * rate) * channels * width // 2))
    return buf.getvalue()


TURNS = [
    {"startMs": 250, "endMs": 1250, "text": "Hello, world"},
    {"startMs": 0, "endMs": 500, "text": ""},
]


class FakeAligner:
    """Spreads each turn's words evenly over its slice. Can block until
    released, fail on a word, or fail to load."""

    def __init__(self):
        self.calls = 0
        self.loads = 0
        self.fail_on = set()
        self.load_error = None
        self.gate = None  # threading.Event the call waits on
        self.entered = threading.Semaphore(0)
        self.active = 0
        self.max_active = 0
        self.temp_files_seen = []
        self._lock = threading.Lock()

    def load(self):
        self.loads += 1
        if self.load_error:
            raise self.load_error

    def align(self, pcm, tokens):
        with self._lock:
            self.calls += 1
            self.active += 1
            self.max_active = max(self.max_active, self.active)
        self.temp_files_seen.append(sorted(os.listdir(tempfile.gettempdir())))
        self.entered.release()
        try:
            if self.gate is not None:
                assert self.gate.wait(10)
            if any(token in self.fail_on for token in tokens):
                raise RuntimeError("boom")
            frames = emission_frames(len(pcm) // 2)
            step = frames // len(tokens)
            return [(i * step, i * step + max(1, step // 2), 0.9) for i in range(len(tokens))]
        finally:
            with self._lock:
                self.active -= 1


def register(client, device_id=DEVICE):
    response = client.post("/v1/devices", json={"device_id": device_id})
    assert response.status_code == 200
    return {"Authorization": f"Bearer {response.json()['token']}"}


@pytest.fixture
def temp_dir(tmp_path, monkeypatch):
    path = tmp_path / "tmp"
    path.mkdir()
    monkeypatch.setattr(tempfile, "tempdir", str(path))
    return path


@pytest.fixture
def fake(client, temp_dir):
    fake = FakeAligner()
    client.app.state.aligner = fake
    return fake


@pytest.fixture
def auth(client, fake):
    return register(client)


def post(client, headers, audio=None, turns=TURNS, files=None):
    if files is None:
        files = {
            "audio": ("clip.wav", make_wav() if audio is None else audio, "audio/wav"),
            "turns": (None, turns if isinstance(turns, (str, bytes)) else json.dumps(turns)),
        }
    return client.post("/v1/align", headers=headers, files=files)


def align_usage(settings, device_id=DEVICE):
    conn = db.connect(settings.db_path)
    try:
        row = conn.execute(
            "SELECT align_seconds_day, align_day FROM devices WHERE device_id = ?", (device_id,)
        ).fetchone()
        return row["align_seconds_day"], row["align_day"]
    finally:
        conn.close()


def set_align_usage(settings, seconds, day, device_id=DEVICE):
    conn = db.connect(settings.db_path)
    try:
        conn.execute(
            "UPDATE devices SET align_seconds_day = ?, align_day = ? WHERE device_id = ?",
            (seconds, day, device_id),
        )
    finally:
        conn.close()


def today():
    return datetime.now(UTC).date().isoformat()


def assert_error(response, status, error, phrase=None):
    assert response.status_code == status, response.text
    body = response.json()
    assert body["error"] == error
    assert not any(dash in body["message"] for dash in DASHES)
    if phrase:
        assert phrase in body["message"]
    return body


# Access.


def test_align_needs_a_device_token(client, fake):
    assert_error(post(client, {}), 401, "unauthorized")
    assert_error(post(client, {"Authorization": "Bearer nope"}), 401, "unauthorized")
    assert fake.calls == 0


def test_any_registered_device_may_align_without_relay_ai(client, auth):
    assert client.get("/v1/devices/me", headers=auth).json()["relay_ai"] is False
    assert post(client, auth).status_code == 200


# Results.


def test_align_returns_words_and_skipped_turns(client, auth):
    response = post(client, auth)
    assert response.status_code == 200, response.text
    body = response.json()
    assert set(body) == {"words", "skipped"}
    assert [w["text"] for w in body["words"]] == ["Hello,", "world"]
    for word in body["words"]:
        assert set(word) == {"text", "startMs", "endMs", "score", "turn"}
        assert isinstance(word["startMs"], int) and isinstance(word["endMs"], int)
        assert word["turn"] == 0 and word["score"] == 0.9
        assert 0 <= word["startMs"] <= word["endMs"] <= 1500
    assert body["skipped"] == [{"turn": 1, "reason": NO_TEXT}]


def test_extra_turn_fields_are_ignored(client, auth):
    turns = [{"turnId": 0, "speaker": "A", "startMs": 250, "endMs": 1250, "text": "hi"}]
    assert post(client, auth, turns=turns).status_code == 200


def test_no_turns_is_an_empty_result(client, auth, fake):
    response = post(client, auth, turns=[])
    assert response.status_code == 200
    assert response.json() == {"words": [], "skipped": []}
    assert align_usage(client.app.state.settings) == (0, None)


def test_a_turn_the_model_fails_on_is_skipped_not_500(client, auth, fake):
    fake.fail_on = {"boom"}
    turns = [{"startMs": 0, "endMs": 900, "text": "fine"}, {"startMs": 1000, "endMs": 1900, "text": "boom"}]
    response = post(client, auth, turns=turns)
    assert response.status_code == 200
    assert response.json()["skipped"] == [{"turn": 1, "reason": FAILED}]
    assert [w["text"] for w in response.json()["words"]] == ["fine"]


def test_a_model_that_cannot_load_is_503_and_not_charged(client, auth, fake):
    fake.load_error = RuntimeError("no model")
    assert_error(post(client, auth), 503, "align_unavailable")
    assert align_usage(client.app.state.settings)[0] == 0


# Daily minutes.


def test_aligned_seconds_are_charged_for_today(client, auth):
    settings = client.app.state.settings
    assert post(client, auth).status_code == 200
    # One turn aligned: 0.25 to 1.25 s plus 0.25 s on each side, clamped at 0.
    assert align_usage(settings) == (pytest.approx(1.5), today())
    assert post(client, auth).status_code == 200
    assert align_usage(settings) == (pytest.approx(3.0), today())


def test_skipped_turns_are_not_charged(client, auth):
    turns = [{"startMs": 0, "endMs": 500, "text": "..."}, {"startMs": 9000, "endMs": 9500, "text": "late"}]
    response = post(client, auth, turns=turns)
    assert response.status_code == 200 and len(response.json()["skipped"]) == 2
    assert align_usage(client.app.state.settings) == (0, None)


def test_used_up_daily_minutes_is_429(client, auth, fake):
    settings = client.app.state.settings
    set_align_usage(settings, 120 * 60, today())
    response = post(client, auth)
    body = assert_error(response, 429, "align_quota")
    assert body["message"] == "This device has used today's 120 minutes of word timing. It resets at midnight UTC."
    assert 0 < int(response.headers["retry-after"]) <= 86400
    assert fake.calls == 0
    assert align_usage(settings) == (120 * 60, today())


def test_daily_minutes_reset_on_a_new_utc_day(client, auth):
    settings = client.app.state.settings
    yesterday = (datetime.now(UTC).date() - timedelta(days=1)).isoformat()
    set_align_usage(settings, 120 * 60, yesterday)
    assert post(client, auth).status_code == 200
    assert align_usage(settings) == (pytest.approx(1.5), today())


def test_a_request_larger_than_the_minutes_left_is_429(client, auth, fake):
    settings = client.app.state.settings
    set_align_usage(settings, 120 * 60 - 1, today())
    # The request needs 1.5 s; 1 s is left.
    response = post(client, auth)
    body = assert_error(response, 429, "align_quota")
    assert body["message"] == (
        "This request needs 2 seconds of word timing and this device has 1 second left today. "
        "Send less audio or try again after midnight UTC."
    )
    assert 0 < int(response.headers["retry-after"]) <= 86400
    assert fake.calls == 0
    assert align_usage(settings) == (120 * 60 - 1, today())


def test_a_request_that_just_fits_the_minutes_left_is_aligned(client, auth):
    settings = client.app.state.settings
    set_align_usage(settings, 120 * 60 - 1.5, today())
    assert post(client, auth).status_code == 200
    assert align_usage(settings)[0] == pytest.approx(120 * 60)
    assert_error(post(client, auth), 429, "align_quota")


def test_failed_turns_give_their_reserved_seconds_back(client, auth, fake):
    fake.fail_on = {"boom"}
    turns = [{"startMs": 250, "endMs": 1250, "text": "fine"}, {"startMs": 1500, "endMs": 1750, "text": "boom"}]
    assert post(client, auth, turns=turns).status_code == 200
    # Only the first turn's 0 to 1.5 s slice is kept.
    assert align_usage(client.app.state.settings) == (pytest.approx(1.5), today())


def test_two_requests_cannot_race_past_the_daily_minutes(client, auth, fake):
    settings = client.app.state.settings
    # Room for one 1.5 s request, not two.
    set_align_usage(settings, 120 * 60 - 2, today())
    fake.gate = threading.Event()
    results = []
    first = run_in_thread(client, auth, results)
    assert fake.entered.acquire(timeout=5)
    assert_error(post(client, auth), 429, "align_quota")
    fake.gate.set()
    first.join(10)
    assert results == [200]
    assert align_usage(settings)[0] == pytest.approx(120 * 60 - 0.5)


def test_turns_covering_the_audio_many_times_are_400_without_model_calls(client, auth, fake):
    turns = [{"startMs": 0, "endMs": 600_000, "text": "a"}] * 300
    body = assert_error(post(client, auth, audio=make_wav(seconds=600), turns=turns), 400, "invalid_request")
    assert body["message"] == (
        "The turns add up to more than the audio. Each turn should cover its own part of the audio, "
        "with little overlap."
    )
    assert fake.calls == 0
    assert align_usage(client.app.state.settings) == (0, None)


def test_turns_may_overlap_a_little(client, auth):
    # Ten 1 s turns over 10 s of audio, each overlapping the next by 0.4 s.
    turns = [{"startMs": i * 1000 - 400 * (i > 0), "endMs": (i + 1) * 1000, "text": "word"} for i in range(10)]
    assert post(client, auth, audio=make_wav(seconds=10), turns=turns).status_code == 200


def test_two_turns_over_the_same_audio_are_400(client, auth, fake):
    turns = [{"startMs": 0, "endMs": 10_000, "text": "one"}, {"startMs": 0, "endMs": 10_000, "text": "two"}]
    assert_error(post(client, auth, audio=make_wav(seconds=10), turns=turns), 400, "invalid_request")
    assert fake.calls == 0


def test_daily_minutes_come_from_the_setting(tmp_path, temp_dir):
    settings = load_settings(
        {"RELAY_DB_PATH": str(tmp_path / "relay.db"), "ALIGN_MINUTES_PER_DEVICE_PER_DAY": "0.5"}
    )
    with TestClient(create_app(settings)) as client:
        client.app.state.aligner = FakeAligner()
        auth = register(client)
        set_align_usage(settings, 30, today())
        body = assert_error(post(client, auth), 429, "align_quota")
        assert "today's 0.5 minutes" in body["message"]


# Upload limits.


@pytest.mark.parametrize(
    ("audio", "phrase"),
    [
        (make_wav(channels=2), "The audio must be mono."),
        (make_wav(rate=44100), "The audio must be 16 kHz."),
        (make_wav(rate=8000), "The audio must be 16 kHz."),
        (make_wav(width=1), "The audio must be 16-bit PCM."),
        (make_wav(width=3), "The audio must be 16-bit PCM."),
        (make_wav(seconds=0), "The audio is empty."),
        (b"ID3\x04\x00\x00\x00\x00\x00\x00" + b"\xff\xfb" * 400, "Send the audio as a WAV file"),
        (b"", "Send the audio as a WAV file"),
        (make_wav()[:30], "Send the audio as a WAV file"),
    ],
    ids=["stereo", "44k", "8k", "8-bit", "24-bit", "empty", "mp3", "no-bytes", "cut-header"],
)
def test_audio_that_is_not_16k_mono_16bit_wav_is_400(client, auth, fake, audio, phrase):
    assert_error(post(client, auth, audio=audio), 400, "invalid_request", phrase)
    assert fake.calls == 0
    assert align_usage(client.app.state.settings) == (0, None)


def test_float_wav_is_400(client, auth):
    header = make_wav(seconds=0.1)
    # Rewrite the format tag from 1 (PCM) to 3 (IEEE float).
    audio = header[:20] + b"\x03\x00" + header[22:]
    assert_error(post(client, auth, audio=audio), 400, "invalid_request", "Send the audio as a WAV file")


def with_chunk_sizes(audio, riff=None, data=None):
    """A WAV header with its RIFF or data size rewritten."""
    out = bytearray(audio)
    if riff is not None:
        out[4:8] = struct.pack("<I", riff)
    if data is not None:
        assert out[36:40] == b"data"
        out[40:44] = struct.pack("<I", data)
    return bytes(out)


@pytest.mark.parametrize(
    "audio",
    [
        make_wav()[:12] + b"JUNK" + struct.pack("<I", 0x7FFFFFF0) + b"\x00" * 8 + make_wav()[12:],
        make_wav()[:16] + struct.pack("<I", 0xFFFFFF00) + make_wav()[20:],
    ],
    ids=["junk-past-riff", "fmt-past-riff"],
)
def test_wav_chunks_running_past_the_file_are_400(client, auth, fake, audio):
    assert_error(post(client, auth, audio=audio), 400, "invalid_request", "Send the audio as a WAV file")
    assert fake.calls == 0


def test_streaming_wav_without_sizes_is_judged_by_the_bytes_present(client, auth):
    # Streaming writers leave the sizes at 0xFFFFFFFF.
    audio = with_chunk_sizes(make_wav(seconds=2), riff=0xFFFFFFFF, data=0xFFFFFFFF)
    assert post(client, auth, audio=audio).status_code == 200
    assert align_usage(client.app.state.settings) == (pytest.approx(1.5), today())


def test_a_header_claiming_more_audio_than_sent_counts_only_what_was_sent(client, auth):
    audio = with_chunk_sizes(make_wav(seconds=0.4), data=0x7FFFFFFE)
    turns = [{"startMs": 0, "endMs": 300_000, "text": "hello"}]
    assert post(client, auth, audio=audio, turns=turns).status_code == 200
    # The slice is clamped to the 0.4 s actually present.
    assert align_usage(client.app.state.settings) == (pytest.approx(0.4), today())


def test_streaming_wav_over_ten_minutes_is_400(client, auth, fake):
    audio = with_chunk_sizes(make_wav(seconds=600.5), riff=0xFFFFFFFF, data=0xFFFFFFFF)
    body = assert_error(post(client, auth, audio=audio), 400, "invalid_request")
    assert body["message"] == "The audio is longer than 10 minutes."


def test_streaming_wav_with_no_samples_is_empty(client, auth):
    audio = with_chunk_sizes(make_wav(seconds=0), data=0xFFFFFFFF)
    assert_error(post(client, auth, audio=audio), 400, "invalid_request", "The audio is empty.")


def test_audio_longer_than_ten_minutes_is_400(client, auth, fake):
    body = assert_error(post(client, auth, audio=make_wav(seconds=600.5)), 400, "invalid_request")
    assert body["message"] == "The audio is longer than 10 minutes."
    assert fake.calls == 0


def test_ten_minutes_of_audio_is_accepted(client, auth):
    assert post(client, auth, audio=make_wav(seconds=600)).status_code == 200


def test_upload_larger_than_ten_minutes_of_audio_is_413(client, auth, fake):
    audio = make_wav(seconds=0.1) + b"\x00" * (21 * 1024 * 1024)
    assert_error(post(client, auth, audio=audio), 413, "too_large")
    assert fake.calls == 0


def turns_of_size(size):
    """One turn whose JSON is exactly `size` bytes."""
    empty = json.dumps([{"startMs": 0, "endMs": 1000, "text": ""}])
    return json.dumps([{"startMs": 0, "endMs": 1000, "text": "a" * (size - len(empty))}])


def test_turns_larger_than_128_kb_are_413(client, auth, fake):
    body = assert_error(post(client, auth, turns=turns_of_size(128 * 1024 + 1)), 413, "too_large")
    assert body["message"] == "The turns are larger than 128 KB."
    assert fake.calls == 0


def test_turns_of_128_kb_are_accepted(client, auth, fake):
    response = post(client, auth, turns=turns_of_size(128 * 1024))
    assert response.status_code == 200
    # Far more letters than 1.5 s of audio can hold.
    assert response.json()["skipped"] == [{"turn": 0, "reason": TOO_SHORT}]


def test_turns_are_parsed_and_planned_off_the_event_loop(client, auth, monkeypatch):
    # Parsing and planning large turns takes a while; on the event loop it
    # would hold up every other request, proxied calls included.
    from relay import align

    seen = []

    def on_event_loop():
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            return False
        return True

    def spy(name, real):
        def wrapper(*args):
            seen.append((name, on_event_loop()))
            return real(*args)

        return wrapper

    monkeypatch.setattr(align, "parse_turns", spy("parse_turns", align.parse_turns))
    monkeypatch.setattr(align, "plan", spy("plan", align.plan))
    assert post(client, auth).status_code == 200
    assert seen == [("parse_turns", False), ("plan", False)]


def test_declared_length_over_the_limit_is_413_before_reading(client, auth):
    response = client.post(
        "/v1/align",
        headers={**auth, "Content-Type": "multipart/form-data; boundary=x", "Content-Length": str(10**9)},
        content=b"",
    )
    assert_error(response, 413, "too_large")


@pytest.mark.parametrize(
    ("turns", "phrase"),
    [
        ("not json", "Send turns as a JSON list"),
        (json.dumps({"startMs": 0, "endMs": 1, "text": "hi"}), "Send turns as a JSON list"),
        (json.dumps([{"startMs": 0, "text": "hi"}]), "Turn 0 is not valid."),
        (json.dumps([{"startMs": 0, "endMs": 900, "text": "ok"}, {"startMs": "soon", "endMs": 1, "text": "hi"}]), "Turn 1 is not valid."),
        (json.dumps([{"startMs": 0.5, "endMs": 900, "text": "hi"}]), "Turn 0 is not valid."),
        (json.dumps([{"startMs": 0, "endMs": 900, "text": 42}]), "Turn 0 is not valid."),
        (json.dumps(["hi"]), "Turn 0 is not valid."),
        (json.dumps([{"startMs": 0, "endMs": 900, "text": "hi"}] * 301), "Send at most 300 turns."),
        ("[" + "1e400" + "]", "Turn 0 is not valid."),
        ("[" * 30_000 + "]" * 30_000, "Send turns as a JSON list"),
    ],
    ids=["not-json", "object", "missing-end", "string-start", "fraction", "text-number", "string-turn", "301", "inf", "deep"],
)
def test_bad_turns_are_400(client, auth, fake, turns, phrase):
    assert_error(post(client, auth, turns=turns), 400, "invalid_request", phrase)
    assert fake.calls == 0


def test_three_hundred_turns_are_accepted(client, auth):
    turns = [{"startMs": i * 1000, "endMs": i * 1000 + 500, "text": "a"} for i in range(300)]
    response = post(client, auth, audio=make_wav(seconds=300), turns=turns)
    assert response.status_code == 200
    assert len(response.json()["words"]) == 300


def test_turns_part_over_one_megabyte_is_413(client, auth):
    turns = json.dumps([{"startMs": 0, "endMs": 900, "text": "word " * 250_000}])
    assert_error(post(client, auth, turns=turns), 413, "too_large")


@pytest.mark.parametrize(
    "files",
    [
        {"audio": ("clip.wav", make_wav(), "audio/wav")},
        {"turns": (None, json.dumps(TURNS))},
        {
            "audio": ("clip.wav", make_wav(), "audio/wav"),
            "turns": (None, json.dumps(TURNS)),
            "extra": (None, "x"),
        },
        [
            ("audio", ("clip.wav", make_wav(), "audio/wav")),
            ("audio", ("clip.wav", make_wav(), "audio/wav")),
            ("turns", (None, json.dumps(TURNS))),
        ],
    ],
    ids=["no-turns", "no-audio", "extra-part", "two-audio"],
)
def test_bad_multipart_is_400(client, auth, fake, files):
    assert_error(post(client, auth, files=files), 400, "invalid_request", "one audio part and one turns part")
    assert fake.calls == 0


def test_a_body_that_is_not_multipart_is_400(client, auth):
    response = client.post("/v1/align", headers=auth, json={"turns": TURNS})
    assert_error(response, 400, "invalid_request", "multipart/form-data")


# Temp files.


def test_temp_file_is_deleted_after_every_request(client, auth, fake, temp_dir):
    assert post(client, auth).status_code == 200
    assert len(fake.temp_files_seen[-1]) == 1  # the audio was on disk while aligning
    assert os.listdir(temp_dir) == []

    assert post(client, auth, audio=make_wav(channels=2)).status_code == 400
    assert os.listdir(temp_dir) == []

    fake.fail_on = {"hello"}
    assert post(client, auth).status_code == 200
    assert os.listdir(temp_dir) == []

    fake.load_error = RuntimeError("no model")
    assert post(client, auth).status_code == 503
    assert os.listdir(temp_dir) == []

    assert post(client, auth, audio=make_wav(seconds=0.1) + b"\x00" * (21 * 1024 * 1024)).status_code == 413
    assert os.listdir(temp_dir) == []


# Concurrency.


def run_in_thread(client, headers, results):
    def go():
        results.append(post(client, headers).status_code)

    thread = threading.Thread(target=go)
    thread.start()
    return thread


def concurrent_app(tmp_path, concurrency):
    settings = load_settings(
        {"RELAY_DB_PATH": str(tmp_path / "relay.db"), "ALIGN_CONCURRENCY": str(concurrency)}
    )
    return TestClient(create_app(settings))


@pytest.mark.parametrize("concurrency", [1, 2])
def test_alignments_run_at_most_align_concurrency_at_a_time(tmp_path, temp_dir, concurrency):
    with concurrent_app(tmp_path, concurrency) as client:
        fake = FakeAligner()
        fake.gate = threading.Event()
        client.app.state.aligner = fake
        devices = [register(client, DEVICE), register(client, OTHER_DEVICE)]
        results = []
        threads = [run_in_thread(client, headers, results) for headers in devices]
        for _ in range(concurrency):
            assert fake.entered.acquire(timeout=5)
        time.sleep(0.3)
        assert fake.active == concurrency
        fake.gate.set()
        for thread in threads:
            thread.join(10)
        assert results == [200, 200]
        assert fake.max_active == concurrency


def test_a_device_gets_at_most_two_alignments_at_once(tmp_path, temp_dir):
    with concurrent_app(tmp_path, 2) as client:
        fake = FakeAligner()
        fake.gate = threading.Event()
        client.app.state.aligner = fake
        headers = register(client)
        results = []
        threads = [run_in_thread(client, headers, results) for _ in range(2)]
        for _ in range(2):
            assert fake.entered.acquire(timeout=5)
        busy = post(client, headers)
        assert_error(busy, 429, "busy")
        # An alignment takes a while: no point retrying within a second.
        assert busy.headers["retry-after"] == "30"
        # Another device still gets in: it waits for a free worker.
        other = register(client, OTHER_DEVICE)
        thread = run_in_thread(client, other, results)
        time.sleep(0.2)
        fake.gate.set()
        for t in [*threads, thread]:
            t.join(10)
        assert results == [200, 200, 200]


def test_a_full_alignment_queue_is_429_with_retry_after(tmp_path, temp_dir):
    with concurrent_app(tmp_path, 1) as client:
        fake = FakeAligner()
        fake.gate = threading.Event()
        client.app.state.aligner = fake
        results = []
        # One worker: 4 requests may be in the relay at once.
        devices = [register(client, str(uuid.uuid4())) for _ in range(5)]
        threads = [run_in_thread(client, headers, results) for headers in devices[:4]]
        assert fake.entered.acquire(timeout=5)
        deadline = time.monotonic() + 5
        while client.app.state.align_in_flight.count() < 4 and time.monotonic() < deadline:
            time.sleep(0.01)
        busy = post(client, devices[4])
        assert_error(busy, 429, "busy", "The relay is busy.")
        assert busy.headers["retry-after"] == "30"
        fake.gate.set()
        for thread in threads:
            thread.join(10)
        assert results == [200] * 4


# Model settings.


def test_the_model_gets_the_thread_setting(tmp_path):
    settings = load_settings({"RELAY_DB_PATH": str(tmp_path / "relay.db"), "ALIGN_THREADS": "5"})
    assert create_app(settings).state.aligner.threads == 5


@pytest.mark.parametrize(("flag", "loads"), [("1", 1), ("0", 0)])
def test_warmup_loads_the_model_at_startup_only_when_enabled(tmp_path, flag, loads):
    settings = load_settings({"RELAY_DB_PATH": str(tmp_path / "relay.db"), "ALIGN_WARMUP": flag})
    app = create_app(settings)
    fake = FakeAligner()
    app.state.aligner = fake
    with TestClient(app) as client:
        deadline = time.monotonic() + 5
        while fake.loads < loads and time.monotonic() < deadline:
            time.sleep(0.01)
        assert client.get("/healthz").status_code == 200
    assert fake.loads == loads


def test_a_failed_warmup_does_not_stop_the_server(tmp_path):
    settings = load_settings({"RELAY_DB_PATH": str(tmp_path / "relay.db"), "ALIGN_WARMUP": "1"})
    app = create_app(settings)
    fake = FakeAligner()
    fake.load_error = RuntimeError("no model")
    app.state.aligner = fake
    with TestClient(app) as client:
        assert client.get("/healthz").status_code == 200


# Clients that hang up.


@pytest.fixture
def anyio_backend():
    return "asyncio"


def multipart_body(audio, turns):
    boundary = uuid.uuid4().hex
    body = (
        f'--{boundary}\r\nContent-Disposition: form-data; name="turns"\r\n\r\n{json.dumps(turns)}\r\n'
        f'--{boundary}\r\nContent-Disposition: form-data; name="audio"; filename="a.wav"\r\n'
        "Content-Type: audio/wav\r\n\r\n"
    ).encode() + audio + f"\r\n--{boundary}--\r\n".encode()
    return body, f"multipart/form-data; boundary={boundary}"


class HangingUpClient:
    """One request straight through the ASGI app; hangs up when told."""

    def __init__(self, app, token, audio, turns):
        self.app = app
        self.body, self.content_type = multipart_body(audio, turns)
        self.token = token
        self.hang_up = anyio.Event()
        self.status = None

    async def run(self):
        body_sent = False

        async def receive():
            nonlocal body_sent
            if not body_sent:
                body_sent = True
                return {"type": "http.request", "body": self.body, "more_body": False}
            await self.hang_up.wait()
            return {"type": "http.disconnect"}

        async def send(message):
            if message["type"] == "http.response.start" and not self.hang_up.is_set():
                self.status = message["status"]

        scope = {
            "type": "http",
            "asgi": {"version": "3.0", "spec_version": "2.3"},
            "http_version": "1.1",
            "method": "POST",
            "scheme": "http",
            "path": "/v1/align",
            "raw_path": b"/v1/align",
            "root_path": "",
            "query_string": b"",
            "headers": [
                (b"authorization", f"Bearer {self.token}".encode()),
                (b"content-type", self.content_type.encode()),
                (b"content-length", str(len(self.body)).encode()),
            ],
            "client": ("127.0.0.1", 50000),
            "server": ("relay", 80),
        }
        with anyio.fail_after(10):
            await self.app(scope, receive, send)


@asynccontextmanager
async def started_app(settings):
    app = create_app(settings)
    async with app.router.lifespan_context(app):
        yield app


def add_device(settings, device_id=DEVICE):
    token = uuid.uuid4().hex
    conn = db.connect(settings.db_path)
    try:
        conn.execute(
            "INSERT INTO devices (device_id, token_hash, created_at) VALUES (?, ?, ?)",
            (device_id, hash_token(token), datetime.now(UTC).isoformat()),
        )
    finally:
        conn.close()
    return token


THREE_TURNS = [
    {"startMs": 250, "endMs": 1000, "text": "one"},
    {"startMs": 2000, "endMs": 3000, "text": "two"},
    {"startMs": 4000, "endMs": 5000, "text": "three"},
]


@pytest.mark.anyio
async def test_a_client_that_hangs_up_stops_the_work_between_turns(settings, temp_dir):
    token = add_device(settings)
    async with started_app(settings) as app:
        fake = FakeAligner()
        fake.gate = threading.Event()
        app.state.aligner = fake
        call = HangingUpClient(app, token, make_wav(seconds=6), THREE_TURNS)
        async with anyio.create_task_group() as tg:
            tg.start_soon(call.run)
            assert await anyio.to_thread.run_sync(fake.entered.acquire, True, 5)
            call.hang_up.set()
            await anyio.sleep(0.1)
            fake.gate.set()
        # Only the first turn ran; the slot is free and the other two turns'
        # reserved seconds are given back.
        assert fake.calls == 1
        assert app.state.align_in_flight.count() == 0
        assert align_usage(settings) == (pytest.approx(1.25), today())
        assert os.listdir(temp_dir) == []


@pytest.mark.anyio
async def test_a_client_that_hangs_up_while_queued_never_reaches_the_model(tmp_path, temp_dir):
    settings = load_settings({"RELAY_DB_PATH": str(tmp_path / "relay.db"), "ALIGN_CONCURRENCY": "1"})
    first, second = add_device(settings), add_device(settings, OTHER_DEVICE)
    async with started_app(settings) as app:
        fake = FakeAligner()
        fake.gate = threading.Event()
        app.state.aligner = fake
        running = HangingUpClient(app, first, make_wav(), TURNS)
        queued = HangingUpClient(app, second, make_wav(seconds=6), THREE_TURNS)
        async with anyio.create_task_group() as tg:
            tg.start_soon(running.run)
            assert await anyio.to_thread.run_sync(fake.entered.acquire, True, 5)
            tg.start_soon(queued.run)
            await anyio.sleep(0.2)
            queued.hang_up.set()
            await anyio.sleep(0.1)
            fake.gate.set()
        assert running.status == 200
        assert fake.calls == 1
        assert align_usage(settings, OTHER_DEVICE)[0] == 0
        assert app.state.align_in_flight.count() == 0


@pytest.mark.anyio
async def test_a_client_that_hangs_up_while_queued_frees_its_slot_and_minutes_at_once(tmp_path, temp_dir):
    settings = load_settings({"RELAY_DB_PATH": str(tmp_path / "relay.db"), "ALIGN_CONCURRENCY": "1"})
    first, second = add_device(settings), add_device(settings, OTHER_DEVICE)
    async with started_app(settings) as app:
        fake = FakeAligner()
        fake.gate = threading.Event()
        app.state.aligner = fake
        running = HangingUpClient(app, first, make_wav(), TURNS)
        queued = HangingUpClient(app, second, make_wav(seconds=6), THREE_TURNS)
        async with anyio.create_task_group() as tg:
            tg.start_soon(running.run)
            assert await anyio.to_thread.run_sync(fake.entered.acquire, True, 5)
            tg.start_soon(queued.run)
            await anyio.sleep(0.2)
            assert app.state.align_in_flight.count(OTHER_DEVICE) == 1
            assert align_usage(settings, OTHER_DEVICE)[0] > 0
            queued.hang_up.set()
            await anyio.sleep(0.2)
            # The first alignment still holds the only worker; the queued one
            # has already left the queue, given back its slot and its minutes.
            try:
                assert app.state.align_in_flight.count(OTHER_DEVICE) == 0
                assert align_usage(settings, OTHER_DEVICE)[0] == 0
                assert len(os.listdir(temp_dir)) == 1
            finally:
                fake.gate.set()
        assert running.status == 200
        assert fake.calls == 1
