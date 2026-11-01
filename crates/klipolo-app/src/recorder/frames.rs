//! The Record dialog's preview frames and events to the page. Frames (the cards' stills, the live picture, the camera's)
//! go as raw bytes over the channel the page gives with `recorder_previews`, never as JSON events: no base64 reaches the
//! WebView, and it runs no script per frame but one small header's parse. Each message is the header's length in two
//! bytes (big-endian), the header (`PreviewFrame`: which picture, what it shows, when the app got it) as UTF-8 JSON,
//! then the JPEG's own bytes, which the page draws from a `Blob`. The mic's peaks, and why a preview stopped by itself,
//! go as `recorder-event`s with the preview's id (`PreviewId`).

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use klipolo_capture::protocol::Event;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};

use super::types::{PreviewFrame, PreviewId, PreviewKind, RecorderEvent};

/// Where the previews' frames and events go: the page (`AppOut`), or a test's record.
pub(super) trait PreviewOut: Send + Sync {
    /// One frame message (`frame_message`). False once the page has gone (its channel closed).
    fn frame(&self, message: Vec<u8>) -> bool;
    /// A preview's own event, as `recorder-event` with its id.
    fn event(&self, id: PreviewId, ev: &Event);
}

/// The message for frame `jpeg_base64` of `kind` showing `key`, got at `at_ms`; None when its base64 isn't (no
/// preview worker of Klipolo's sends that) or its header doesn't fit in two bytes.
pub(super) fn frame_message(
    kind: PreviewKind,
    key: &str,
    at_ms: u64,
    jpeg_base64: &str,
) -> Option<Vec<u8>> {
    let header = PreviewFrame {
        kind,
        key: key.to_owned(),
        at_ms,
    };
    let header = serde_json::to_vec(&header).ok()?;
    let length = u16::try_from(header.len()).ok()?;
    let mut message = Vec::with_capacity(2 + header.len() + jpeg_base64.len() / 4 * 3);
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(&header);
    STANDARD.decode_vec(jpeg_base64, &mut message).ok()?;
    Some(message)
}

/// Now, in milliseconds since 1970: when a frame was got.
pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The page: frames over the channel of the dialog's last `recorder_previews`, events as `recorder-event`.
pub(super) struct AppOut {
    app: AppHandle,
    frames: Channel<InvokeResponseBody>,
}

impl AppOut {
    pub(super) fn new(app: AppHandle, frames: Channel<InvokeResponseBody>) -> Self {
        Self { app, frames }
    }
}

impl PreviewOut for AppOut {
    fn frame(&self, message: Vec<u8>) -> bool {
        self.frames.send(InvokeResponseBody::Raw(message)).is_ok()
    }

    fn event(&self, id: PreviewId, ev: &Event) {
        let payload = RecorderEvent {
            take: id.as_str().to_owned(),
            event: ev.clone(),
        };
        if let Err(error) = self.app.emit("recorder-event", payload) {
            tracing::warn!(%error, "recorder: a preview's event didn't reach the page");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_its_header_s_length_its_header_then_its_jpeg_s_own_bytes() {
        let jpeg = [0xff, 0xd8, 0x00, 0x7f, 0xff, 0xd9];
        let message = frame_message(
            PreviewKind::Thumb,
            "display:1",
            1_761_900_000_123,
            &STANDARD.encode(jpeg),
        )
        .expect("a message");
        let length = usize::from(u16::from_be_bytes([message[0], message[1]]));
        let header: serde_json::Value = serde_json::from_slice(&message[2..2 + length]).unwrap();
        assert_eq!(
            header,
            serde_json::json!({ "kind": "thumb", "key": "display:1", "atMs": 1_761_900_000_123_u64 })
        );
        assert_eq!(&message[2 + length..], jpeg);
        assert_eq!(
            frame_message(PreviewKind::Live, "display:1", 0, "not base64!"),
            None
        );
        // A long key still fits: the header's length takes two bytes.
        let long = "k".repeat(1_000);
        let message = frame_message(PreviewKind::Camera, &long, 0, "").unwrap();
        assert!(u16::from_be_bytes([message[0], message[1]]) > 1_000);
    }

    #[test]
    fn each_preview_s_id_is_what_the_page_reads() {
        for id in [
            PreviewId::Thumbs,
            PreviewId::Live,
            PreviewId::Camera,
            PreviewId::Mic,
        ] {
            assert_eq!(
                serde_json::to_string(&id).unwrap(),
                format!("\"{}\"", id.as_str())
            );
        }
    }
}
