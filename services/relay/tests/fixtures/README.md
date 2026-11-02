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

## `fleurs_hi_pyramids_16k.wav`, `fleurs_ar_pyramids_16k.wav` and their `.json`

The same sentence read in Hindi (5.58 s) and in Egyptian Arabic (5.16 s), one
speaker each: "You can see the pyramids in the dark and in silence before the
show starts."

- **Source:** FLEURS (Conneau et al., 2022, "FLEURS: Few-shot Learning
  Evaluation of Universal Representations of Speech"), the Hugging Face dataset
  `google/fleurs` (https://huggingface.co/datasets/google/fleurs) at revision
  `70bb2e84b976b7e960aa89f1c648e09c59f894dd`, dev
  split, sentence id 1537: `data/hi_in/audio/dev.tar.gz` file
  `14112322580063897119.wav` (sha256 `f4d34913...c112bc`) and
  `data/ar_eg/audio/dev.tar.gz` file `2768129653097135459.wav` (sha256
  `32d806ec...c6c541`).
- **License:** the recordings are CC BY 4.0
  (https://creativecommons.org/licenses/by/4.0/), as the dataset card states.
  Attribution: Google, FLEURS.
- **Transcript text:** the sentence in each `.json` is FLEURS's transcript,
  which is FLoRes-101's sentence (Goyal et al., 2021, "The FLORES-101
  Evaluation Benchmark for Low-Resource and Multilingual Machine Translation",
  https://github.com/facebookresearch/flores), under CC BY-SA 4.0
  (https://creativecommons.org/licenses/by-sa/4.0/). Attribution: Meta AI,
  FLoRes-101.
- **Format:** WAV, 16 kHz, mono, 16-bit PCM. The originals are 16 kHz mono
  32-bit float; ffmpeg converted them to 16-bit PCM without other changes.

Each `.json` holds one turn covering the whole clip, with the dataset's own
transcript as its text, and `reference`: word times from an earlier run of
this aligner (torchaudio MMS_FA 2.8.0) on the uroman-romanized words. As with
the ISS answer, the test checks the relay reproduces that run. Every word
scored at least 0.87 (Hindi) and 0.66 (Arabic), with medians of 0.985 and
0.931.
