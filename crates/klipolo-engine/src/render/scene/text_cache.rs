//! The scene drawer's text: per scene instance, target size, layout scale and
//! fonts' epoch, its text laid out and its atlas drawn on the text build thread (`text_worker.rs`), then uploaded on
//! the render thread (its atlas and its draws' buffer, `text_builds.rs`; collected at each frame's start). The layout
//! scale is the layer's largest (`TextAt`), so a scaling layer's frames all find one entry. A frame that finds none
//! asks for its build and, in the preview, draws the latest entry of the same text meanwhile (another size, older
//! fonts, an earlier value of its settings: fitted evenly, and only from a size within 4× of its own) or
//! none; an export waits for the build. The instance takes the units of whichever entry it draws (`draw.rs`).
//!
//! **Bounded:** per format (the canvas), at most [`MAX_TEXT_SCENES`] entries and [`MAX_TEXT_ATLAS_BYTES`] of atlases,
//! draws and layouts; past either, that format's entries go (drawn ones off screen first, then prepared ones, the oldest
//! first). Never one on screen (drawn last frame) or used this frame: while every entry of a
//! format shows, the format passes its caps, as the instances' do, until one leaves the screen. Each atlas is within
//! the budget: text past it at its softest is refused on the build thread, never kept. Entries expire as instances do
//! (8 frames drawn, 480 prepared); all go on `forget`. At most [`MAX_IN_FLIGHT`] builds are asked at once. Surveys
//! asked alone (a scene without text: `text_builds.rs`) wait for their instance at most 16 at once and
//! for as long as a prepared entry does.

use std::sync::Arc;

use klipolo_model::scene::timeline::CompiledScene;

use super::SceneAt;
use super::sizes::Sizes;
use super::text_builds::Pending;
use super::text_layouts::{ATLAS_BUDGET, TextLayouts};
use super::text_pipeline::{TextGpu, TextPipeline};
use super::text_worker::TextWorker;
use crate::plan::SceneLayer;

/// The most scene instances whose text a format keeps.
pub const MAX_TEXT_SCENES: usize = 4;
/// The most bytes a format's text (atlases, draws and layouts) may hold together.
pub const MAX_TEXT_ATLAS_BYTES: u64 = ATLAS_BUDGET;
/// Builds asked of the text build thread at once.
pub const MAX_IN_FLIGHT: usize = 4;
/// Surveys landed and not taken kept at once.
const MAX_SURVEYS: usize = 16;
const EXPIRE_AFTER_FRAMES: u64 = 8;
const PREPARED_FRAMES: u64 = 480;

/// How a frame's scene text is drawn: laid out at `k` target pixels per 1080p pixel (the frame's pixels × the largest
/// scale the layer's motion reaches: the same every frame); `wait`, an export's: its fonts wait for this
/// computer's; `block`: the frame waits for its build (an export's frame), else it draws without it until it
/// lands (the preview, and every build ahead of its frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextAt {
    pub k: f64,
    pub wait: bool,
    pub block: bool,
}

/// Whose text an entry is: its layer, compiled scene, settings and text language.
pub(super) struct Whose {
    layer: Arc<str>,
    scene: Arc<CompiledScene>,
    params: u64,
    language: Arc<str>,
}

impl Whose {
    pub(super) fn of(layer: &SceneLayer) -> Self {
        Self {
            layer: layer.id.clone(),
            scene: layer.scene.clone(),
            params: layer.params_hash,
            language: layer.language.clone(),
        }
    }

    pub(super) fn is(&self, layer: &SceneLayer) -> bool {
        self.params == layer.params_hash && self.text_of(layer)
    }

    /// The same layer's text, whatever its settings (what a stand-in may be while an edit builds).
    fn text_of(&self, layer: &SceneLayer) -> bool {
        Arc::ptr_eq(&self.scene, &layer.scene)
            && *self.layer == *layer.id
            && *self.language == *layer.language
    }
}

/// Whether text laid out in a target of `laid` may stand in for one of `now`: both a few pixels at least and
/// their long sides within 4× (a window resize, a box dragged), never magnified from a layer that was a few pixels.
fn near((lw, lh): (u32, u32), (nw, nh): (u32, u32)) -> bool {
    let (laid, now) = (lw.max(lh), nw.max(nh));
    lw.min(lh) >= 4
        && nw.min(nh) >= 4
        && laid.saturating_mul(4) >= now
        && now.saturating_mul(4) >= laid
}

/// Where and how it was laid out: its target, its scale, the fonts' epoch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Place {
    pub at: SceneAt,
    pub k: u64,
    pub epoch: u64,
}

/// One instance's text at one place: its layouts, and its atlas and draws on the GPU (None: refused, or no text).
pub(super) struct TextEntry {
    pub(super) whose: Whose,
    pub(super) place: Place,
    /// Unique among entries: the instance re-takes the units of an entry it hasn't drawn before.
    pub id: u64,
    pub layouts: TextLayouts,
    /// Its instance's survey, made on the build thread (`sizes_built.rs`), when it could be.
    pub survey: Option<Sizes>,
    pub gpu: Option<TextGpu>,
    pub(super) bytes: u64,
    pub drawn: bool,
    pub(super) used: u64,
}

/// Whether an entry is on screen or in use: drawn last frame, or used this one. Room is never
/// made by taking it.
fn kept(e: &TextEntry, now: u64) -> bool {
    e.used == now || (e.drawn && now.saturating_sub(e.used) <= 1)
}

/// The order entries go in when room is needed (the smallest first), as the instances' (`cache.rs`): drawn ones off
/// screen, the oldest first; then prepared ones.
fn evict_rank(drawn: bool, used: u64) -> (u8, u64) {
    (u8::from(!drawn), used)
}

#[derive(Default)]
pub(super) struct TextCache {
    entries: Vec<TextEntry>,
    pub(super) pending: Vec<Pending>,
    /// Surveys asked alone and not landed (their ids), and those landed and not taken yet (their ids, the survey if it
    /// could be made there, and the frame each landed).
    pub(super) surveys_asked: Vec<u64>,
    pub(super) surveys: Vec<(u64, Option<Sizes>, u64)>,
    pub(super) worker: Option<TextWorker>,
    pub(super) next_id: u64,
    /// The build thread is gone (it failed to start, or stopped): nothing it was asked will land.
    pub(super) gone: bool,
    /// Bumped when the fonts change (`SceneDrawer::fonts_changed`): every text is built again, the old drawn meanwhile.
    pub(super) epoch: u64,
    /// Tests: how long each build waits first (`SceneDrawer::delay_text_builds`).
    #[cfg(any(test, feature = "test-support"))]
    pub(super) delay: std::time::Duration,
}

/// The GPU and the text pipeline an entry is uploaded with.
pub(super) type Upload<'a> = (&'a wgpu::Device, &'a wgpu::Queue, &'a TextPipeline);

impl TextCache {
    pub(super) fn find(&mut self, layer: &SceneLayer, place: Place, now: u64) -> Option<usize> {
        let i = self
            .entries
            .iter()
            .position(|e| e.place == place && e.whose.is(layer))?;
        self.entries[i].used = now;
        Some(i)
    }

    /// The latest entry of `layer`'s text in the same format at any other place (another size, older fonts, an
    /// earlier value of its settings), from a size near its own: what the preview draws until its own lands.
    pub(super) fn fallback(&mut self, layer: &SceneLayer, at: SceneAt, now: u64) -> Option<usize> {
        let (i, _) = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.place.at.canvas == at.canvas
                    && near(e.place.at.size, at.size)
                    && e.whose.text_of(layer)
            })
            .max_by_key(|(_, e)| (e.used, e.id))?;
        self.entries[i].used = now;
        Some(i)
    }

    pub(super) fn get(&self, i: usize) -> &TextEntry {
        &self.entries[i]
    }

    pub(super) fn get_mut(&mut self, i: usize) -> &mut TextEntry {
        &mut self.entries[i]
    }

    /// Keeps `entry`, first making room in its format. Its index.
    pub(super) fn insert(&mut self, mut entry: TextEntry, now: u64) -> usize {
        entry.used = now;
        let canvas = entry.place.at.canvas;
        loop {
            let same = self.entries.iter().filter(|e| e.place.at.canvas == canvas);
            let (count, bytes) = same.fold((0, 0), |(n, b), e| (n + 1, b + e.bytes));
            if count < MAX_TEXT_SCENES && bytes + entry.bytes <= MAX_TEXT_ATLAS_BYTES {
                break;
            }
            let Some(oldest) = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.place.at.canvas == canvas && !kept(e, now))
                .min_by_key(|(_, e)| evict_rank(e.drawn, e.used))
                .map(|(i, _)| i)
            else {
                break;
            };
            self.entries.swap_remove(oldest);
        }
        self.entries.push(entry);
        self.entries.len() - 1
    }

    /// Keeps survey `id` that landed at frame `now` for its instance (the oldest go past `MAX_SURVEYS`).
    pub(super) fn keep_survey(&mut self, id: u64, survey: Option<Sizes>, now: u64) {
        if self.surveys.len() >= MAX_SURVEYS {
            self.surveys.remove(0);
        }
        self.surveys.push((id, survey, now));
    }

    /// The frame's start: drawn entries unused for a while go, prepared ones wait for their first frame a while.
    pub(super) fn expire(&mut self, now: u64) {
        self.surveys
            .retain(|&(_, _, landed)| now.saturating_sub(landed) <= PREPARED_FRAMES);
        self.entries.retain(|e| {
            let wait = if e.drawn {
                EXPIRE_AFTER_FRAMES
            } else {
                PREPARED_FRAMES
            };
            now - e.used <= wait
        });
    }

    /// Every entry goes, and a build asked before is dropped when it lands (the thread stays; one that is gone goes,
    /// so the next build starts another: closing and opening the project again tries again).
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.pending.clear();
        self.surveys_asked.clear();
        self.surveys.clear();
        if self.gone {
            self.worker = None;
            self.gone = false;
        }
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The bytes every format's text holds.
    pub(super) fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }
}
