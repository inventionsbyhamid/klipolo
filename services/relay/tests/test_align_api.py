"""POST /v1/align with a fake model: auth, daily minutes, upload limits,
temp files and concurrency. No torch needed."""

import io
import json
import os
import struct
import tempfile
import threading
import time
import wave
from datetime import UTC, datetime, timedelta

import pytest
from fastapi.testclient import TestClient

from relay import db
from relay.align import FAILED, NO_TEXT, emission_frames
from relay.app import create_app
from relay.config import load_settings

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
    assert align_usage(client.app.state.settings) == (0, None)


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


def test_a_request_may_start_with_minutes_left_and_go_over(client, auth):
    settings = client.app.state.settings
    set_align_usage(settings, 120 * 60 - 1, today())
    assert post(client, auth).status_code == 200
    assert align_usage(settings)[0] == pytest.approx(120 * 60 + 0.5)
    assert_error(post(client, auth), 429, "align_quota")


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
        ("[" * 100_000 + "]" * 100_000, "Send turns as a JSON list"),
    ],
    ids=["not-json", "object", "missing-end", "string-start", "fraction", "text-number", "string-turn", "301", "inf", "deep"],
)
def test_bad_turns_are_400(client, auth, fake, turns, phrase):
    assert_error(post(client, auth, turns=turns), 400, "invalid_request", phrase)
    assert fake.calls == 0


def test_three_hundred_turns_are_accepted(client, auth):
    turns = [{"startMs": i * 6, "endMs": i * 6 + 5, "text": "a"} for i in range(300)]
    assert post(client, auth, turns=turns).status_code == 200


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
        assert_error(post(client, headers), 429, "busy")
        # Another device still gets in: it waits for a free worker.
        other = register(client, OTHER_DEVICE)
        thread = run_in_thread(client, other, results)
        time.sleep(0.2)
        fake.gate.set()
        for t in [*threads, thread]:
            t.join(10)
        assert results == [200, 200, 200]


# Model settings.


def test_the_model_gets_the_thread_setting(tmp_path):
    settings = load_settings({"RELAY_DB_PATH": str(tmp_path / "relay.db"), "ALIGN_THREADS": "3"})
    assert create_app(settings).state.aligner.threads == 3


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
