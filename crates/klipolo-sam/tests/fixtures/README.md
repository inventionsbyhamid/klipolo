# SAM 3.1 output fixtures

Real responses from Meta's `sam-3.1` model on `POST /v1/responses`, recorded on 2026-10-30.

## Footage

NASA, "SpaceX Crew-1 Astronaut Walkout" (Wikimedia Commons,
`File:NASA's_SpaceX_Crew-1-_Astronaut_Walkout_9hw6SBiJ2Ns.webm`, 480p transcode),
excerpt 46 to 62 s, re-encoded to 854x480 at 10 fps: 160 frames.
NASA media is not copyrighted (public domain).

## Files

| File | What |
|---|---|
| `person_concept_one_bit.txt` | Final output text for the concept prompt "person", `mask_encoding: "one_bit"`, `include_confidence: "true"`. Unchanged. |
| `person_concept_one_bit.boxes.json` | Expected boxes for it: `[frame, id, left, top, right, bottom, confidence]`, half-open source pixels. |
| `box_prompt_f80_lossless.txt` | Final output text for a box prompt on one astronaut at frame 80, propagated both ways, non-streaming, `include_confidence: "true"`. The masks came back lossless (`~`). Each mask payload is cut to its first 3,000 characters to keep the file under 1 MB, so these masks do not decode. Everything else is unchanged. |
| `box_prompt_f80_lossless.boxes.json` | Expected boxes for it, same layout. |

The expected boxes come from Meta's reference parser (`meta-sam-parser` 0.1.0) run on the full,
untrimmed responses. It turns SAM's inclusive `x2`, `y2` into half-open `right = x2 + 1`,
`bottom = y2 + 1`.
