//! The draft layer's projects (`DraftLayer::project` and `baseline`) and what they draw on: the filters' test image in
//! the worker's `tmp/`, and a transition's two sides, served by `FrozenStills`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::time::{Us, secs_to_us};
use klipolo_model::timeline::{
    Clip, ClipSource, EffectInstance, Item, ItemContent, Track, TrackKind, Transition,
};

use super::{DRAFT_SECONDS, DraftLayer, SIDE_A, SIDE_B, TEST_IMAGE, TEST_SIZE, TRANSITION_US};
use crate::media::frozen::FrozenStills;
use crate::registry::ShaderKind;

impl DraftLayer {
    pub(super) fn build(&self, with_pass: bool) -> Project {
        let mut p = Project::new("Draft");
        p.settings.primary_format = self.format;
        p.settings.formats = vec![self.format];
        let length = secs_to_us(self.seconds);
        let params = self.params.clone();
        match self.kind {
            ShaderKind::Generator => {
                if with_pass {
                    let content = ItemContent::generator(self.def.id.clone(), params);
                    p.timeline.tracks =
                        vec![track(item("draft", length, whole(), content, Vec::new()))];
                }
            }
            ShaderKind::Filter => {
                p.assets = vec![asset(
                    "test",
                    AssetKind::Image,
                    &self.image.to_string_lossy(),
                )];
                let effects = with_pass
                    .then(|| EffectInstance {
                        id: "draft".into(),
                        params,
                        ..EffectInstance::new(self.def.id.clone())
                    })
                    .into_iter()
                    .collect();
                let image = ItemContent::Image {
                    asset_id: "test".into(),
                };
                let at = BoxN {
                    x: 0.15,
                    y: 0.15,
                    w: 0.7,
                    h: 0.7,
                };
                p.timeline.tracks = vec![track(item("image", length, at, image, effects))];
            }
            ShaderKind::Transition => {
                p.assets = vec![
                    asset(SIDE_A, AssetKind::Video, SIDE_A),
                    asset(SIDE_B, AssetKind::Video, SIDE_B),
                ];
                let mut a = Clip::new(ClipSource::Asset { id: SIDE_A.into() }, 0, cut());
                let b = Clip::new(ClipSource::Asset { id: SIDE_B.into() }, 0, length - cut());
                if with_pass {
                    a.transition_out = Some(Transition {
                        effect: self.def.id.clone(),
                        duration_us: TRANSITION_US,
                        params,
                    });
                }
                p.timeline.main = vec![a, b];
            }
        }
        p
    }
}

pub(super) fn cut() -> Us {
    secs_to_us(DRAFT_SECONDS / 2.0)
}

fn whole() -> BoxN {
    BoxN {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    }
}

fn item(
    id: &str,
    length: Us,
    at: BoxN,
    content: ItemContent,
    effects: Vec<EffectInstance>,
) -> Item {
    Item {
        id: id.into(),
        start_us: 0,
        duration_us: length,
        attach: None,
        layout: Layout::new(Anchor::TopLeft, at),
        content,
        effects,
    }
}

fn track(item: Item) -> Track {
    let mut t = Track::new(TrackKind::Overlay, "Draft");
    t.id = "draft".into();
    t.items = vec![item];
    t
}

fn asset(id: &str, kind: AssetKind, path: &str) -> Asset {
    Asset {
        id: id.into(),
        kind,
        path: path.into(),
        name: id.into(),
        duration_us: secs_to_us(60.0),
        width: TEST_SIZE.0,
        height: TEST_SIZE.1,
        fps: if kind == AssetKind::Video { 30.0 } else { 0.0 },
        rotation: Some(0),
        has_audio: false,
        audio_lead_us: None,
    }
}

/// 80 px squares of 50 % grey and white.
fn test_pixels() -> image::RgbaImage {
    image::RgbaImage::from_fn(TEST_SIZE.0, TEST_SIZE.1, |x, y| {
        let v = if (x / 80 + y / 80) % 2 == 0 { 128 } else { 255 };
        image::Rgba([v, v, v, 255])
    })
}

/// The test image in `tmp`, written the first time.
pub(super) fn test_image(tmp: &Path) -> std::io::Result<Arc<Path>> {
    let path: PathBuf = tmp.join(TEST_IMAGE);
    if !path.is_file() {
        // A name of its own, so two drafts made at once never write or rename each other's file.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = tmp.join(format!(".new-{}-{n}-{TEST_IMAGE}", std::process::id()));
        test_pixels()
            .save_with_format(&temp, image::ImageFormat::Png)
            .map_err(std::io::Error::other)?;
        std::fs::rename(&temp, &path)?;
    }
    Ok(path.into())
}

/// A transition's two sides: the test image, and its negative.
pub(super) fn transition_stills() -> FrozenStills {
    let a = test_pixels();
    let mut b = a.clone();
    for p in b.pixels_mut() {
        p.0 = [255 - p.0[0] / 2, 255 - p.0[1], 255 - p.0[2] / 3, 255];
    }
    let mut stills = FrozenStills::new();
    stills.insert(SIDE_A, None, a);
    stills.insert(SIDE_B, None, b);
    stills
}
