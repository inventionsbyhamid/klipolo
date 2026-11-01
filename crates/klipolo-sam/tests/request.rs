use klipolo_sam::{concept_request, MODEL};
use serde_json::json;

#[test]
fn concept_request_matches_doc_shape() {
    let body = concept_request("file-abc", "person");
    assert_eq!(MODEL, "sam-3.1");
    assert_eq!(
        body,
        json!({
            "model": "sam-3.1",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "person"},
                    {"type": "input_video", "file_id": "file-abc"}
                ]
            }],
            "metadata": {"include_confidence": "true", "mask_encoding": "one_bit"}
        })
    );
    // One JSON answer, never a stream: the reader takes the whole text at once.
    assert!(body.get("stream").is_none());
}

#[test]
fn concept_request_trims_the_phrase() {
    let body = concept_request("f", "  man in red \n");
    assert_eq!(body["input"][0]["content"][0]["text"], "man in red");
}
