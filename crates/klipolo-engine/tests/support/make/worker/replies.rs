//! What the worker answered, as the tests read it: a compile's reply and its draft, a thumbnail, a frame.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use klipolo_engine::make::budget::Cost;
use klipolo_engine::make::protocol::{DraftSpec, FrameStats, WorkerReply};
use klipolo_engine::registry::guard::Diagnostic;

/// What a compile answered.
pub struct CompileReply {
    pub(super) lines: Vec<String>,
    pub(super) compiled: Option<CompiledView>,
}

/// A `Compiled` reply, with the draft it compiled (for its frames).
pub struct CompiledView {
    pub ok: bool,
    pub cost: Option<Cost>,
    pub diagnostics: Vec<Diagnostic>,
    pub draft: DraftSpec,
}

/// A thumbnail line's base64 text.
pub struct Thumb(String);

impl Thumb {
    pub fn decode(&self) -> Result<Vec<u8>, String> {
        BASE64.decode(&self.0).map_err(|e| e.to_string())
    }
}

impl CompileReply {
    /// Every line it answered, as written.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn compiled(&self) -> Option<&CompiledView> {
        self.compiled.as_ref()
    }

    /// Why it failed, when its final line is `Failed`.
    pub fn failure(&self) -> Option<String> {
        self.lines
            .iter()
            .find_map(|l| match serde_json::from_str(l).ok()? {
                WorkerReply::Failed { reason, .. } => Some(reason),
                _ => None,
            })
    }

    pub fn thumbnail(&self) -> Option<Thumb> {
        self.lines.iter().find_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v["op"] == "thumbnail").then(|| Thumb(v["jpeg"].as_str().unwrap().to_owned()))
        })
    }
}

/// A frame the worker sent.
pub struct FrameView {
    pub stats: FrameStats,
    pub jpeg: Vec<u8>,
}

impl FrameView {
    /// Its pixels, RGB rows.
    pub fn jpeg_decoded(&self) -> Vec<u8> {
        image::load_from_memory(&self.jpeg)
            .unwrap()
            .to_rgb8()
            .into_raw()
    }
}

/// One pixel of a frame.
pub fn pixel_of(frame: &FrameView, (x, y): (u32, u32)) -> [u8; 3] {
    let image = image::load_from_memory(&frame.jpeg).unwrap().to_rgb8();
    image.get_pixel(x, y).0
}
