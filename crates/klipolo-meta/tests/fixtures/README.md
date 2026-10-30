# Test fixtures

- `asr_diarization.json`: a real `muse-voice-transcribe-1.0` answer (DIARIZATION mode, `languageBias: ["English"]`) for 155 s of NASA's "Space Station Astronauts Discuss Life In Space With New York Students, May 20, 2025" (https://images.nasa.gov, excerpt 645 to 800 s). NASA media is public domain.
- `responses_json_schema.json`: a real `muse-spark-1.3` Responses API answer (strict `json_schema` output, reasoning effort low) for a "find the moment" prompt on the same NASA clip; the spike's own `_latency_s` and `_request` fields removed.
- `responses_function_calls.json`: hand-made in the Responses API shape (reasoning, message and two `function_call` items) from one real planner turn: its calls, text and `usage` numbers are the ones `muse-spark-1.3` returned.
