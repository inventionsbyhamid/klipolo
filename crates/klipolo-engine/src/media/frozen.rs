//! The render worker's decode source: stills the app decoded, never a file of the person's. The app
//! fills a folder in the worker's `in/` with, for each requested time, the frame of every video source the frame plan
//! needs (a PNG at most 1024 px on its long edge), plus copies of the image assets used, and an index (`stills.json`,
//! [`StillsIndex`]). `FrozenStills` serves the still for `(asset, source time)` in place of the decoder pool: no FFmpeg,
//! no VideoToolbox, no file outside `in/`. Keyed by the time in the source the layer shows, not the timeline's, so two
//! layers of one file at one moment (a transition across a cut inside one recording) each get their own frame. A
//! frame with no still for one of its videos draws without that video and names
//! it as missing, as the preview does for a file that isn't there. A draft's own stills (a transition's two sides) are
//! the same for every time.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;

use image::ImageFormat;
use klipolo_model::time::{Us, secs_to_us};
use serde::{Deserialize, Serialize};

use crate::make::protocol::InputName;
use crate::plan::FramePlan;
use crate::render::pool::DecodedSet;
use crate::vendor::cap::rendering::decoder::DecodedFrame;

/// The longest edge of a still the app hands over.
pub const MAX_STILL_EDGE: u32 = 1024;
/// A still's file, at most (a PNG of 1024 × 1024 RGBA is under 4 MiB; anything bigger isn't one of the app's).
const MAX_STILL_BYTES: u64 = 8 << 20;
/// The index's file name in the stills folder.
pub const STILLS_INDEX: &str = "stills.json";
const MAX_INDEX_BYTES: u64 = 256 << 10;
/// Stills one folder may hold (9 times × a few sources each).
const MAX_STILLS: usize = 64;

/// What the app put in the stills folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StillsIndex {
    /// A video source's frame at a requested time (seconds on the timeline).
    #[serde(default)]
    pub videos: Vec<VideoStill>,
    /// An image asset's copy (larger than 32 MiB: downscaled by the app first).
    #[serde(default)]
    pub images: Vec<AssetFile>,
    /// A `.cube` LUT the project's looks read.
    #[serde(default)]
    pub luts: Vec<AssetFile>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoStill {
    pub asset: String,
    /// The time in the source (seconds, its own clock) the frame shows: `SourceRef::time_us`.
    pub source_at: f64,
    pub file: InputName,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetFile {
    /// The asset's (or LUT's) id in the snapshot.
    pub id: String,
    pub file: InputName,
}

/// Stills by (asset id, source time); a time of None serves every time.
#[derive(Default)]
pub struct FrozenStills {
    stills: HashMap<(Arc<str>, Option<Us>), DecodedFrame>,
}

impl FrozenStills {
    pub fn new() -> Self {
        Self::default()
    }

    /// `image` for `asset` at its own time `at` (None: at every time).
    pub fn insert(&mut self, asset: &str, at: Option<Us>, image: image::RgbaImage) {
        let (w, h) = image.dimensions();
        self.stills.insert(
            (asset.into(), at),
            DecodedFrame::new(image.into_raw(), w, h),
        );
    }

    /// The video stills `index` names in `folder` (the worker's own `in/<stills>`), each a PNG within
    /// `MAX_STILL_EDGE`. A still that isn't one is an error: the app made them.
    pub fn load(folder: &Path, index: &StillsIndex) -> Result<Self, String> {
        if index.videos.len() > MAX_STILLS {
            return Err(format!("more than {MAX_STILLS} stills"));
        }
        let mut stills = Self::new();
        for v in &index.videos {
            let image = read_png(&folder.join(v.file.as_str()), MAX_STILL_EDGE)?;
            stills.insert(&v.asset, Some(secs_to_us(v.source_at)), image);
        }
        Ok(stills)
    }

    /// The frames `plan`'s video layers show, in `FramePlan::videos()` order, into `set` (reused): the still for each
    /// source at the time it shows, else the one for every time, else none (the source is named in `set.missing`).
    pub fn fill(&self, plan: &FramePlan, set: &mut DecodedSet) {
        set.frames.clear();
        set.warnings.clear();
        set.missing.clear();
        set.not_answering.clear();
        for video in plan.videos() {
            let id = &video.source.asset_id;
            let frame = self
                .stills
                .get(&(id.clone(), Some(video.source.time_us)))
                .or_else(|| self.stills.get(&(id.clone(), None)));
            match frame {
                Some(frame) => set.frames.push(Some((frame.clone(), 0))),
                None => {
                    set.frames.push(None);
                    if !set.missing.contains(&video.source.original) {
                        set.missing.push(video.source.original.clone());
                    }
                }
            }
        }
    }
}

/// The index in `folder`.
pub fn read_index(folder: &Path) -> Result<StillsIndex, String> {
    let mut text = Vec::new();
    std::fs::File::open(folder.join(STILLS_INDEX))
        .and_then(|f| f.take(MAX_INDEX_BYTES + 1).read_to_end(&mut text))
        .map_err(|e| format!("the stills index can't be read: {e}"))?;
    if text.len() as u64 > MAX_INDEX_BYTES {
        return Err("the stills index is too large".into());
    }
    serde_json::from_slice(&text).map_err(|e| format!("the stills index isn't valid: {e}"))
}

/// A PNG of at most `edge` on both sides, decoded within that.
pub fn read_png(path: &Path, edge: u32) -> Result<image::RgbaImage, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_STILL_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("a still can't be read: {e}"))?;
    if bytes.len() as u64 > MAX_STILL_BYTES {
        return Err("a still is too large".into());
    }
    let mut reader = image::io::Reader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(edge);
    limits.max_image_height = Some(edge);
    limits.max_alloc = Some(u64::from(edge) * u64::from(edge) * 4 + (1 << 20));
    reader.limits(limits);
    reader
        .decode()
        .map(|i| i.to_rgba8())
        .map_err(|e| format!("a still isn't a PNG within {edge} px: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use klipolo_model::contrib::builtin;
    use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
    use klipolo_model::timeline::{Clip, ClipSource, Transition};
    use klipolo_model::{Op, apply_ops};

    use super::*;
    use crate::plan::{PlanContext, frame_plan};

    /// Two clips of one recording, a dissolve across their cut.
    fn one_file_cut() -> Project {
        let p = Project::new("Cut");
        let asset = Asset {
            id: "a1".into(),
            kind: AssetKind::Video,
            path: "/elsewhere/rec.mp4".into(),
            name: "rec.mp4".into(),
            duration_us: 10_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: Some(0),
            has_audio: false,
            audio_lead_us: None,
        };
        let clip = |id: &str, from: i64, to: i64| Clip {
            id: id.into(),
            ..Clip::new(ClipSource::Asset { id: "a1".into() }, from, to)
        };
        let transition = Transition {
            effect: builtin::TRANSITION_DISSOLVE.into(),
            duration_us: 1_000_000,
            params: Default::default(),
        };
        apply_ops(
            &p,
            &[
                Op::AddAssets {
                    assets: vec![asset],
                },
                Op::InsertClip {
                    clip: clip("c1", 0, 4_000_000),
                    index: None,
                },
                Op::InsertClip {
                    clip: clip("c2", 6_000_000, 10_000_000),
                    index: None,
                },
                Op::SetTransition {
                    clip_id: "c1".into(),
                    transition: Some(transition),
                },
            ],
        )
        .unwrap()
    }

    /// Across a cut inside one file, both sides of the transition are the same asset at the same timeline
    /// time; each is served the still of the time it shows in the source.
    #[test]
    fn two_layers_of_one_file_get_their_own_stills() {
        let ctx = PlanContext::new(Arc::new(one_file_cut()));
        let plan = (35..45)
            .map(|tenth| frame_plan(&ctx, tenth * 100_000, FormatKind::Landscape))
            .find(|p| p.videos().count() == 2)
            .expect("a frame inside the dissolve");
        let times: Vec<Us> = plan.videos().map(|v| v.source.time_us).collect();
        assert_ne!(times[0], times[1], "different source times");
        let mut stills = FrozenStills::new();
        for (t, shade) in times.iter().zip([40u8, 200]) {
            stills.insert(
                "a1",
                Some(*t),
                image::RgbaImage::from_pixel(4, 4, image::Rgba([shade, shade, shade, 255])),
            );
        }
        let mut set = DecodedSet::default();
        stills.fill(&plan, &mut set);
        let first = |i: usize| set.frames[i].as_ref().map(|(f, _)| f.data()[0]);
        assert_eq!((first(0), first(1)), (Some(40), Some(200)));
        assert!(set.missing.is_empty());
    }
}
