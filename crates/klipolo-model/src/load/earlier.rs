//! Projects saved before the cursor record named a recording's cursor file `cursorPath`. One field read, not a
//! migration: it becomes the recording's `cursor`, read as a take of that time (burned in, sampled clicks), and the
//! writes that shape.

use serde_json::Value;

use crate::cursor_style::RecordingCursor;

const KEY: &str = "cursorPath";

/// Whether `json` may hold the earlier field. A SIMD scan of the text (NEON on the Mac) for the key itself: the writer
/// is compact, so a key is always followed by `:`, and a string value that is exactly `cursorPath` can't match. Every
/// other file still parses once.
pub(super) fn names_cursor_paths(json: &str) -> bool {
    memchr::memmem::find(json.as_bytes(), b"\"cursorPath\":").is_some()
}

/// Each recording's `cursorPath` becomes its `cursor`, unless it already has one.
pub(super) fn read_cursor_paths(project: &mut Value) {
    let Some(recordings) = project.get_mut("recordings").and_then(Value::as_array_mut) else {
        return;
    };
    for recording in recordings.iter_mut().filter_map(Value::as_object_mut) {
        let Some(Value::String(path)) = recording.remove(KEY) else {
            continue;
        };
        if recording.contains_key("cursor") {
            continue;
        }
        if let Ok(cursor) = serde_json::to_value(RecordingCursor::burned_in(path)) {
            recording.insert("cursor".into(), cursor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::names_cursor_paths;

    #[test]
    fn only_the_key_takes_the_earlier_path_not_a_text_that_says_it() {
        assert!(names_cursor_paths(
            r#"{"recordings":[{"cursorPath":"/r/cursor.jsonl"}]}"#
        ));
        assert!(!names_cursor_paths(r#"{"layers":[{"text":"cursorPath"}]}"#));
    }
}
