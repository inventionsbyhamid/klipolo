//! The request SAM 3.1 takes for a video: one short noun phrase and the video by its uploaded file's id, answered once
//! (not streamed) with every match's box and confidence on every frame. Masks come back as one bit a pixel, the
//! smallest answer; the reader skips them.

use serde_json::{json, Value};

/// The model's id on Meta's Responses API.
pub const MODEL: &str = "sam-3.1";

/// The Responses API body asking SAM to find and track every `phrase` ("person", "man in red") in the video uploaded
/// as `file_id`. One phrase a request: lists, negation and places ("on the left") are not understood.
pub fn concept_request(file_id: &str, phrase: &str) -> Value {
    json!({
        "model": MODEL,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": phrase.trim()},
                {"type": "input_video", "file_id": file_id}
            ]
        }],
        "metadata": {"include_confidence": "true", "mask_encoding": "one_bit"}
    })
}
