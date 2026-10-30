# Test fixtures

## `iss_answer_16k.wav` and `iss_answer.json`

25.8 seconds of speech: JAXA astronaut Takuya Onishi on the International Space
Station answering a question about challenges during missions. Only the
astronaut speaks in this excerpt.

- **Source:** NASA, "Space Station Astronauts Discuss Life In Space With New York
  Students, May 20, 2025",
  https://images.nasa.gov/details/Iss073m261401418_Space_Station_Astronauts_Discuss_Life_In_Space_With_New_York_Students_250520
  (file `~small.mp4`). The excerpt runs from 744.0 s to 769.8 s of that file.
- **License:** public domain. NASA media is generally not subject to copyright
  in the United States (NASA media usage guidelines). No NASA logo or
  endorsement is implied.
- **Format:** WAV, 16 kHz, mono, 16-bit PCM. The NASA video's audio was
  converted to this format with ffmpeg, and this excerpt was then cut from it
  with Python's `wave` module.

`iss_answer.json` holds:

- `turns`: the speech split into two turns at a 0.84 s pause. The text is
  what Meta's speech-to-text returned for this audio, including its invented
  repeats ("for the for the", "as a as a"). Turn times are in ms from the
  start of the WAV. The second turn ends past the end of the audio on purpose.
- `reference`: word times from an earlier run of this same aligner
  (torchaudio MMS_FA 2.8.0), not from a human or a different tool. That run
  aligned the whole answer as one turn; its times are shifted to this excerpt.
  So the test checks that the relay reproduces that run, not that the times
  are right. The earlier run was checked against the audio itself: word
  starts after a pause landed a median 19 ms after the rise in energy, 80%
  within 100 ms. Words scoring below 0.05 are the invented repeats; their
  times are not meaningful.
