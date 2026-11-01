"""Makes klipolo-chat's test fixtures, beside this script, from clip A's analysis fixtures and a Spark moments answer.

usage: python3 -I make_fixtures.py <spark moments answer.json>
The Spark answer is a whole Responses API answer whose message text is the moments JSON (start_sec, end_sec, kind,
who, description), as a first trial of the planner kept it (not in this repository). Writes transcript_clip_a.json,
turns_clip_a.json, scenes_clip_a.json and project_two_clips.json.
The transcript follows klipolo-analysis's `transcript::assemble`: aligned words scoring under 0.05 dropped, fillers
tagged, each word ending by the next one's start, and a pause word in every gap of 700 ms or more.
"""
import json
import os
import sys

out = os.path.dirname(os.path.abspath(__file__))
fx_in = os.path.normpath(os.path.join(out, "../../../klipolo-analysis/tests/fixtures"))
spark_answer = sys.argv[1]

MIN_SCORE = 0.05
PAUSE_US = 700_000
FILLERS = {"um", "umm", "uh", "uhh", "uhm", "er", "erm", "ah", "hm", "hmm", "mm", "mhm"}

asr = json.load(open(os.path.join(fx_in, "asr_clip_a.json")))
align = json.load(open(os.path.join(fx_in, "align_clip_a.json")))
duration_us = asr["audioDurationMs"] * 1000

def is_filler(text):
    bare = "".join(c for c in text if c.isalpha()).lower()
    return bare in FILLERS

words = []
for w in align["words"]:
    if w["score"] < MIN_SCORE:
        continue
    words.append({
        "text": w["text"],
        "startUs": min(max(w["startMs"] * 1000, 0), duration_us),
        "endUs": min(max(w["endMs"] * 1000, 0), duration_us),
        "confidence": w["score"],
        "kind": "filler" if is_filler(w["text"]) else "word",
    })
words.sort(key=lambda w: w["startUs"])
for i in range(1, len(words)):
    words[i - 1]["endUs"] = min(words[i - 1]["endUs"], words[i]["startUs"])
words = [w for w in words if w["endUs"] > w["startUs"]]
assembled = []
for w in words:
    if assembled and w["startUs"] - assembled[-1]["endUs"] >= PAUSE_US:
        assembled.append({"text": "", "startUs": assembled[-1]["endUs"], "endUs": w["startUs"], "confidence": 1.0,
                          "kind": "pause"})
    assembled.append(w)

transcript = {
    "key": "clip-a",
    "language": "en",
    "model": "muse-voice-transcribe-1.0",
    "coveredUs": duration_us,
    "complete": True,
    "words": assembled,
}
turns = [{"startUs": t["startMs"] * 1000, "endUs": t["endMs"] * 1000, "speaker": t.get("speaker")} for t in asr["turns"]]

spark = json.load(open(spark_answer))
text = next(c["text"] for o in spark["output"] if o["type"] == "message" for c in o["content"])
moments = json.loads(text)["moments"]
scenes = {"items": [{
    "startUs": round(m["start_sec"] * 1_000_000),
    "endUs": round(m["end_sec"] * 1_000_000),
    "kind": m["kind"],
    "who": m["who"],
    "text": m["description"],
    "onScreenText": None,
} for m in moments]}

look = {"preset": None, "strength": 1.0, "lutId": None, "lutStrength": 1.0}
def clip(cid, src_in, src_out):
    return {"id": cid, "source": {"type": "asset", "id": "clip-a"}, "sourceInUs": src_in, "sourceOutUs": src_out,
            "speed": 1.0, "volume": 1.0, "muted": False, "fadeInUs": 0, "fadeOutUs": 0, "look": look}
project = {
    "version": 7,
    "id": "space-station",
    "name": "Space station Q&A",
    "settings": {"fps": 30, "formats": ["landscape", "portrait"], "primaryFormat": "portrait"},
    "assets": [{"id": "clip-a", "kind": "video", "path": "/media/astronauts.mp4", "name": "astronauts.mp4",
                "durationUs": duration_us, "width": 1280, "height": 720, "fps": 29.97, "hasAudio": True}],
    # One cut: 13.0 to 24.0 s of the file (the first student's question) is gone, so the clip is two pieces.
    "timeline": {"main": [clip("clip-a1", 0, 13_000_000), clip("clip-a1-2", 24_000_000, duration_us)], "tracks": []},
    "captions": {"enabled": True, "style": "klipolo.builtin/caption.karaoke"},
}

def dump(name, value):
    with open(os.path.join(out, name), "w") as f:
        json.dump(value, f, indent=1, ensure_ascii=False)
        f.write("\n")

dump("transcript_clip_a.json", transcript)
dump("turns_clip_a.json", turns)
dump("scenes_clip_a.json", scenes)
dump("project_two_clips.json", project)
print(len(assembled), "words incl.", sum(w["kind"] == "pause" for w in assembled), "pauses;", len(turns), "turns;",
      len(scenes["items"]), "scene items")
