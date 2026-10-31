//! A preview frame as the WebView gets it: raw bytes over the app's channel, with no base64, no
//! JSON and no script for the WebView to run per frame. One byte with the length of the id its worker's events carry
//! (`live-3`, `check-1`, a take's id), that id in UTF-8, then the JPEG's own bytes. The worker still sends the frame to
//! the app as a `Preview` event (base64 in its JSON line); the app decodes it once, into the message.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// The message for the frame `jpeg_base64` of worker `id`; None when the id is longer than 255 bytes or the base64
/// isn't (neither comes from Klipolo's own workers).
pub fn preview_wire(id: &str, jpeg_base64: &str) -> Option<Vec<u8>> {
    let id = id.as_bytes();
    let len = u8::try_from(id.len()).ok()?;
    let mut out = Vec::with_capacity(1 + id.len() + jpeg_base64.len() / 4 * 3);
    out.push(len);
    out.extend_from_slice(id);
    STANDARD.decode_vec(jpeg_base64, &mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_its_id_then_its_jpeg_s_own_bytes() {
        let jpeg = [0xff, 0xd8, 0x00, 0x7f, 0xff, 0xd9];
        let wire = preview_wire("live-12", &STANDARD.encode(jpeg)).expect("a message");
        assert_eq!(wire[0], 7);
        assert_eq!(&wire[1..8], b"live-12");
        assert_eq!(&wire[8..], jpeg);
        assert_eq!(preview_wire("live-1", "not base64!"), None);
        assert_eq!(preview_wire(&"x".repeat(256), ""), None);
    }
}
