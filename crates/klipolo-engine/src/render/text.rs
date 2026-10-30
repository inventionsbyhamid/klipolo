//! Text layers. Cap's glyphon text layer (vendored) lays each text out into its own transparent texture
//! at output resolution, vertically centred in its box like the DOM stage. The compositor then draws that texture
//! through the image quad, so text can rotate, fade, slide and pop. A texture is drawn again only when the text, its
//! style or its size in pixels changes: fades, slides and pops reuse it, and an unchanged frame allocates nothing. A
//! text whose motion grows it (scale keys, an elastic zoom) is laid out once at about its largest size
//! (`layout_scale`), with mipmaps, and drawn smaller until it grows, so it is sharp at every scale and never laid out
//! per frame.
//! One glyph atlas serves every text a builder draws (`build`; the compositor drops the drawer on reset); it is trimmed
//! after each text, so it holds only the glyphs of the text being drawn. The preview never waits for the system font
//! scan: until it is done, texts draw with Klipolo's own fonts only (an installed font draws its stand-in) and are drawn
//! again when it finishes, as they are whenever a font is registered (`fonts::generation`). The font system is the
//! compositor's one (`SharedFonts`), which the caption drawer shares; it tells the drawer when the faces
//! changed (`fonts_changed`). A drawer made on its own (`new`: tests) keeps its own.
//!
//! Built ahead, kept briefly: the playback lookahead hands the drawer each title about a
//! second before it enters (`ahead`), and the title build thread (`worker`) shapes it and draws its texture there, so
//! the render thread only draws it. A title whose build hasn't landed by its first frame waits for it (never built
//! twice); one no one asked for (a cold seek, a scrub onto it, an export's frames) is built here, as before. A title's
//! texture stays `RETAIN_FRAMES` frames after it was last drawn (a scrub back across it builds nothing), and a build
//! from the lookahead waits up to `PREBUILT_FRAMES` for its first frame.
//!
//! Fonts: a text is shaped with exactly the face `klipolo_fonts::resolve` picks for its font, weight and
//! style, and its Chinese, Japanese or Korean stretches ask for their language's face (`faces`). A frame drawing a
//! stand-in says `frame.fontMissing`, and one drawing CJK whose Noto Sans isn't downloaded `frame.cjkFontMissing`.
//!
//! Bounded: a text far bigger than the frame (a box of 4 × 4 canvases) gets a texture of only the
//! part that can show, and all text textures together stay within the ceiling, `MAX_TEXT_BYTES` in the preview and
//! `EXPORT_TEXT_BYTES` per exported format (less what the caption atlases hold: `reserve`), the extra ones laid out
//! softer. A new text is tried at full resolution first while its box fits the ceiling (its texture is cut to its ink).
//! Textures kept for titles not in the frame count toward it too, and go oldest first when a title of the frame needs
//! their room: they never make one softer. Builds asked ahead count toward later asks and toward what a landing build
//! may keep, never against a title on screen, so it passes the ceiling by at most their estimates, and
//! only until they land. Captions aren't texts: the caption renderer draws them (`render::captions`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arrayvec::ArrayVec;
use tokio::sync::Notify;

use crate::plan::{TextLayer, TextOwner};
use crate::render::shared_fonts::SharedFonts;
use crate::vendor::cap::compat::XY;
use crate::vendor::cap::rendering::layers::image::ImageQuad;
use crate::vendor::cap::rendering::layers::{fonts_scanned, new_font_system};

mod ahead;
mod build;
pub(crate) mod faces;
mod geometry;
mod worker;
mod wrap;
use build::{Key, Layout, TitleBuilder};
use faces::Said;
pub use geometry::layout_scale;
use geometry::{Geometry, Try, Window, Windows, window};
use worker::Worker;
#[doc(hidden)]
pub use worker::{build_times, time_builds};

/// Drop-shadow strength when a text's shadow is on (Cap's first text preset).
pub const SHADOW: f32 = 0.35;
/// Line height as a multiple of the font size (Cap's default; close to CSS `normal`).
const LINE_HEIGHT: f32 = 1.2;
/// The most bytes the preview's text textures may hold at once: past it, a text is laid out at a
/// lower resolution and magnified (so softer) instead, never allocated past it. About a dozen full-frame 1080p texts,
/// or three at 4K (the shared memory of an Intel Mac's GPU is the app's too; textures are
/// cut to their ink, and the preview is rarely 4K).
pub const MAX_TEXT_BYTES: u64 = 96 << 20;
/// An export's ceiling: 256 MiB per exported format, as before. Each format a pass draws has its
/// own compositor and text drawer, so a pass of k formats may hold up to k × 256 MiB of text textures in the export
/// process. Each draws one frame at a time and builds nothing ahead, and a softened text stops the export
/// (`exports.frame.tooManyTexts`), so it keeps the room 4K frames of big texts need.
pub const EXPORT_TEXT_BYTES: u64 = 256 << 20;
/// How many frames a title's texture is kept after the last frame that drew it: a scrub back and forth
/// across a title's start or end draws from it instead of building it again.
pub const RETAIN_FRAMES: u64 = 8;
/// How many frames a title the lookahead built waits for its first frame (its lookahead is a second: 60 frames at
/// 60 fps, more at 2×).
const PREBUILT_FRAMES: u64 = 150;
/// Builds asked of the title build thread at once (the lookahead's second rarely holds more titles).
const MAX_IN_FLIGHT: usize = 4;
/// The longest a frame waits for a title's build that hasn't landed before building it itself.
const WAIT_AT_MOST: Duration = Duration::from_millis(250);
/// The most bytes any text drawer's textures have held at once in this process (a diagnostic, for the ceiling's test).
static PEAK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// See [`PEAK`].
pub fn text_texture_peak() -> u64 {
    PEAK.load(std::sync::atomic::Ordering::Relaxed)
}

/// Starts [`text_texture_peak`] again (a test measuring one frame).
#[doc(hidden)]
pub fn reset_text_texture_peak() {
    PEAK.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// What a frame says once when it drew texts softer, or left one out, for its drawer's ceiling (96 MiB,
/// [`MAX_TEXT_BYTES`], in the preview; [`EXPORT_TEXT_BYTES`] in an export). A code: an export stops on it, so a text is
/// never missing from a finished export.
pub const TEXT_OVER_BUDGET: &str = crate::api::FRAME_TEXTS_SOFTER;
/// How long a text that didn't fit, or a softened one whose sharper layout didn't, waits before it is tried again
///: never a layout every frame.
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(1);
/// How many times a text has been laid out (shaped) in this process (a diagnostic: the ceiling's test checks that a
/// steady frame lays nothing out).
static LAYOUTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// See [`LAYOUTS`].
#[doc(hidden)]
pub fn text_layouts() -> u64 {
    LAYOUTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// What a drawer's titles cost and hold (tests and the perf harness).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TitleStats {
    /// Titles shaped, and textures drawn, on the thread that draws frames (a layout and a draw count one each).
    pub built_here: u64,
    /// Builds asked of the title build thread and not landed yet.
    pub in_flight: usize,
    /// The bytes the title textures held take, and how many titles the drawer holds.
    pub bytes: u64,
    pub held: usize,
    /// The estimated bytes of the builds in flight.
    pub in_flight_bytes: u64,
    /// The glyph layer's text areas in the last title it drew here (each stamped copy is one: what an outline costs),
    /// and how many times its glyph atlas was made anew.
    pub glyph_areas: usize,
    pub atlas_resets: u64,
    /// Outline glyphs the glyph layer made since its atlas was last made anew, and the outline images it stroked in all
    /// (each once, however often its atlas grows).
    pub outline_glyphs: usize,
    pub outline_strokes: u64,
    /// Titles built ahead touched once on the frame they landed (`take_touch`).
    pub touched: u64,
}

/// A text that didn't fit within its drawer's ceiling (96 MiB in the preview, [`EXPORT_TEXT_BYTES`] in an export) even
/// at its softest: not drawn, and not laid out again until [`RETRY_AFTER`] or a change.
struct Refused {
    whole: (u32, (u32, u32)),
    text: Arc<str>,
    retry_at: std::time::Instant,
    /// The last frame whose plan has it (`begin_frame` keeps it while it does).
    seen: u64,
}

/// What one try at a resolution came to.
enum Attempt {
    Drawn(wgpu::Texture, ImageQuad, f64),
    /// Nothing of it shows this frame.
    Hidden,
    /// Its texture would take these bytes, more than its room.
    TooBig(u64),
}

struct Drawn {
    key: Key,
    layout: Layout,
    /// Its texture and the part of the layout it holds; None while none of it shows (its layout is kept
    /// M-2).
    drawn: Option<(Window, wgpu::Texture)>,
    /// The drawer's frame count when this text was last prepared (or, built ahead, when it landed).
    frame: u64,
    /// Its texture's bytes (mipmaps included); 0 without one.
    bytes: u64,
    /// The share of its full resolution it is laid out at (1, ½, ¼…) for the drawer's ceiling (96 MiB in the preview,
    /// [`EXPORT_TEXT_BYTES`] in an export), and the size at 1.
    down: f64,
    whole: (u32, (u32, u32)),
    /// When a softened text may try its sharper layout again (after one that didn't fit).
    retry_at: Option<std::time::Instant>,
    /// Its sharpest retry that didn't fit, and the bytes it needed: not laid out again while the room is smaller (a
    /// full-resolution layout a second otherwise, for as long as it shows soft).
    too_big: Option<(f64, u64)>,
    /// The last frame whose plan has it (`begin_frame`).
    seen: u64,
    /// Built ahead by the title build thread and not drawn yet: kept [`PREBUILT_FRAMES`], not [`RETAIN_FRAMES`].
    ahead: bool,
}

impl Drawn {
    /// Whether it is still kept at `frame` though no plan has it.
    fn kept_at(&self, frame: u64) -> bool {
        let keep = if self.ahead {
            PREBUILT_FRAMES
        } else {
            RETAIN_FRAMES
        };
        frame.saturating_sub(self.frame) <= keep
    }
}

impl Geometry {
    fn key(&self, layer: &TextLayer, font_px: u32, box_px: (u32, u32)) -> Key {
        Key {
            text: layer.text.clone(),
            style: layer.style.clone(),
            font_px,
            box_px,
            language: layer.language.clone(),
            pad: layer.pad_px.to_bits(),
        }
    }

    /// Where its layer is placed this frame (its motion's scale and turn), laid out at `scale` texture pixels per box
    /// pixel.
    fn place(&self, scale: f64) -> Place {
        Place {
            centre: self.centre,
            k: self.scale / scale,
            rotation: self.rotation,
            out: self.out,
        }
    }
}

/// Where a frame places a text's layout: its centre on screen, screen pixels per texture pixel, its turn and the
/// output, which say what part of a text far bigger than the frame can show (`geometry::window`).
#[derive(Clone, Copy)]
struct Place {
    centre: (f64, f64),
    k: f64,
    rotation: f64,
    out: (f64, f64),
}

/// The windows of `layout` a texture holds when placed at `place`, cut to its ink (never the box's
/// empty room); None when nothing of it shows.
fn shown(layout: &Layout, place: &Place) -> Option<Windows> {
    let windows = window(
        layout.full,
        place.centre,
        place.k,
        place.rotation,
        place.out,
    )?;
    let Some(ink) = layout.ink else {
        return Some(windows);
    };
    Some(Windows {
        visible: windows.visible.within(ink)?,
        cut: windows.cut.within(ink)?,
    })
}

/// Where a text's texture is kept: its owner and id (`TextOwner`), so a layer the person named "captions" never shares
/// the captions' texture.
type Slot = (TextOwner, Arc<str>);

fn slot(layer: &TextLayer) -> Slot {
    (layer.owner, layer.id.clone())
}

/// A build asked of the title build thread and not landed yet.
struct Asked {
    /// Its request's number, which the build lands with.
    ticket: u64,
    slot: Slot,
    key: Key,
    whole: (u32, (u32, u32)),
    /// Its texture's estimated bytes (`Geometry::estimate`).
    bytes: u64,
    /// No longer wanted (a frame stopped waiting for it, or the fonts changed): never waited for, and let go when it
    /// lands, but counted until then, as the thread makes its texture all the same.
    dropped: bool,
}

pub struct TextDrawer {
    /// What builds a title here (a cold seek, an export): its glyph layer, faces and mipmaps.
    builder: TitleBuilder,
    drawn: HashMap<Slot, Drawn>,
    /// What this frame said about fonts (once a frame each).
    said: Said,
    frame: u64,
    /// A drawer made on its own (`new`) keeps its own fonts; the compositor's shares its `SharedFonts`.
    own: Option<SharedFonts>,
    /// This frame drew texts softer for the drawer's `ceiling`, or left one out, and said so.
    softer_said: bool,
    refused: HashMap<Slot, Refused>,
    /// Bytes of the drawer's `ceiling` other drawers hold (the caption atlases: `reserve`).
    reserved: u64,
    /// The title build thread, started by the first `ahead`; `gone` once it can't be (it panicked, or never started).
    worker: Option<Worker>,
    gone: bool,
    in_flight: ArrayVec<Asked, MAX_IN_FLIGHT>,
    /// The next request's number (`Asked::ticket`).
    tickets: u64,
    /// Textures of builds that landed this frame, to touch once (`take_touch`), and how many were.
    touches: ArrayVec<wgpu::Texture, MAX_IN_FLIGHT>,
    touched: u64,
    /// Bumped by `fonts_changed`: a build asked before it is dropped when it lands.
    epoch: u64,
    /// The most bytes this drawer's textures held (`TitleStats`).
    peak: u64,
    /// Its ceiling: [`MAX_TEXT_BYTES`], or an export's [`EXPORT_TEXT_BYTES`] (`for_export`).
    ceiling: u64,
    /// How long a frame waits for a build asked ahead (`WAIT_AT_MOST`; a test's `wait_at_most`).
    wait: Duration,
}

impl TextDrawer {
    /// Scans the system fonts on a thread (hundreds of milliseconds on macOS), so no frame waits for it; every font
    /// system after that clones the scanned database. `redraw` fires when the scan is done.
    pub fn scan_fonts_in_background(redraw: Arc<Notify>) {
        if fonts_scanned() {
            return;
        }
        std::thread::Builder::new()
            .name("klipolo-fonts".into())
            .spawn(move || {
                let delay = crate::render::shared_fonts::background_scan_delay();
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                drop(new_font_system());
                redraw.notify_one();
            })
            .ok();
    }

    /// A drawer with its own fonts (tests): with `wait` the font scan is waited for, else it draws with Klipolo's own
    /// fonts until the scan is done. `prepare` draws with them.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, wait: bool) -> Self {
        Self {
            own: Some(SharedFonts::new(wait)),
            ..Self::shared(device, queue)
        }
    }

    /// A drawer of the compositor's: it draws with the `SharedFonts` each `prepare_with` is given.
    pub fn shared(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            builder: TitleBuilder::new(device, queue),
            drawn: HashMap::new(),
            said: Said::default(),
            frame: 0,
            own: None,
            softer_said: false,
            refused: HashMap::new(),
            reserved: 0,
            worker: None,
            gone: false,
            in_flight: ArrayVec::new(),
            tickets: 0,
            touches: ArrayVec::new(),
            touched: 0,
            epoch: 0,
            peak: 0,
            ceiling: MAX_TEXT_BYTES,
            wait: WAIT_AT_MOST,
        }
    }

    /// How long a frame waits for a build asked ahead before building it itself (tests: so the late
    /// path runs whatever the machine's speed).
    #[doc(hidden)]
    pub fn wait_at_most(&mut self, wait: Duration) {
        self.wait = wait;
    }

    /// The drawer of an export (`export`: the compositor waits for its assets) has an export's ceiling,
    /// [`EXPORT_TEXT_BYTES`]; the preview's keeps [`MAX_TEXT_BYTES`]. The compositor makes a new drawer whenever it
    /// changes between the two (`Compositor::set_wait_for_assets`).
    pub fn for_export(mut self, export: bool) -> Self {
        self.ceiling = if export {
            EXPORT_TEXT_BYTES
        } else {
            MAX_TEXT_BYTES
        };
        self
    }

    /// Bytes of the drawer's ceiling (96 MiB in the preview, [`EXPORT_TEXT_BYTES`] in an export) held elsewhere this
    /// frame (the caption drawer's atlases): the texts get the rest.
    pub fn reserve(&mut self, bytes: u64) {
        self.reserved = bytes;
    }

    /// Makes the pipelines a text can need (glyphs are made in `new`; text boxes on first use) ahead of time.
    pub fn warm(&mut self, device: &wgpu::Device) {
        self.builder.warm(device);
    }

    /// The perf harness: a title built here writes its GPU time into `stamps`, or stops.
    pub(crate) fn time_gpu(&mut self, stamps: Option<Arc<crate::render::text_timing::Stamps>>) {
        self.builder.time_gpu(stamps);
    }

    /// Marks `layer` as one of the coming frame's texts (`begin_frame`).
    pub fn in_frame(&mut self, layer: &TextLayer) {
        let frame = self.frame;
        let slot = slot(layer);
        if let Some(drawn) = self.drawn.get_mut(&slot) {
            drawn.seen = frame;
        }
        if let Some(refused) = self.refused.get_mut(&slot) {
            refused.seen = frame;
        }
    }

    /// Starts a frame once its texts are marked (`in_frame`): the builds that landed are kept, and textures of texts
    /// no longer kept go (`RETAIN_FRAMES`, `PREBUILT_FRAMES`). Only this frame's texts need room under the drawer's
    /// ceiling (96 MiB in the preview, [`EXPORT_TEXT_BYTES`] in an export): those kept for other texts give theirs up
    /// first (`make_room`), so a cut never softens the new scene's texts for the last one's, nor fails an export for it
    ///.
    ///
    /// A refused text's wait is kept while the frame has it, and the softer note can be said again.
    pub fn begin_frame(&mut self) {
        self.drain();
        self.expire();
        let frame = self.frame;
        self.refused.retain(|_, refused| refused.seen == frame);
        self.softer_said = false;
    }

    /// A title built ahead that landed this frame, to touch now: the compositor draws it once, a
    /// pixel at zero opacity, so the GPU has used it (made it resident, sampled it through the quad's pipeline) a
    /// second or so before its first frame rather than on it. Each built texture is given once.
    pub(crate) fn take_touch(&mut self) -> Option<wgpu::Texture> {
        let texture = self.touches.pop()?;
        self.touched += 1;
        Some(texture)
    }

    /// Ends a frame.
    pub fn end_frame(&mut self) {
        // Touches not taken (a drawer the compositor doesn't draw, in tests) are let go.
        self.touches.clear();
        self.said = Said::default();
        self.expire();
        self.frame += 1;
    }

    /// Drops what no frame needs any more: textures past their keep (a drawer `new` makes, which no frame marks, too).
    fn expire(&mut self) {
        let frame = self.frame;
        self.drawn
            .retain(|_, drawn| drawn.seen == frame || drawn.kept_at(frame));
    }

    /// Under memory pressure: only this frame's textures are kept.
    pub fn trim(&mut self) {
        let frame = self.frame;
        self.drawn.retain(|_, drawn| drawn.seen == frame);
    }

    /// The bytes the text textures held now take.
    fn held_bytes(&self) -> u64 {
        self.drawn.values().map(|d| d.bytes).sum()
    }

    /// What this drawer's titles cost and hold (tests, the perf harness).
    #[doc(hidden)]
    pub fn stats(&self) -> TitleStats {
        TitleStats {
            built_here: self.builder.builds(),
            in_flight: self.in_flight.len(),
            in_flight_bytes: self.in_flight_bytes(),
            bytes: self.held_bytes(),
            held: self.drawn.len(),
            glyph_areas: self.builder.glyphs().areas_last_prepare(),
            atlas_resets: self.builder.glyphs().atlas_resets(),
            outline_glyphs: self.builder.glyphs().outline_counts().0,
            outline_strokes: self.builder.glyphs().outline_counts().1,
            touched: self.touched,
        }
    }

    /// The most bytes this drawer's textures have held (tests).
    #[doc(hidden)]
    pub fn peak_bytes(&self) -> u64 {
        self.peak
    }

    fn note_peak(&mut self) {
        let held = self.held_bytes();
        self.peak = self.peak.max(held);
        PEAK.fetch_max(held, std::sync::atomic::Ordering::Relaxed);
    }

    /// The texture of `layer` in an output of `out` pixels (the plan's canvas is `canvas`), where it goes, and how
    /// many times bigger than its box it is laid out (`layout_scale`: texture pixels per drawn pixel at scale 1), or
    /// None when there is nothing to draw.
    ///
    /// Bounded: a text much bigger than the frame gets a texture of the part that can show
    /// (`window`), and every text texture held stays within the drawer's ceiling ([`MAX_TEXT_BYTES`] in the preview, an
    /// export's [`EXPORT_TEXT_BYTES`]). Past it, a text is laid out at the largest resolution its estimate fits (½, ¼…
    /// down to about a texel) and magnified, said once a frame in `warnings`, and keeps it: a softened
    /// text tries its sharper layout at most once a second, and one that doesn't fit even at its softest is left out,
    /// said every frame it is missing from, and waits as long. A steady frame lays nothing out,
    /// nor does a text wholly off the frame.
    ///
    /// Only for a drawer made with `new` (tests), which draws with its own fonts, following their changes as the
    /// compositor does for its; it panics on the compositor's drawer (`shared`), which draws with `prepare_with`.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer: &TextLayer,
        canvas: (u32, u32),
        out: (u32, u32),
        warnings: &mut Vec<String>,
    ) -> Option<(wgpu::Texture, ImageQuad, f64)> {
        let mut fonts = self.own.take().expect("a drawer made with `new`");
        if fonts.refresh() {
            self.fonts_changed();
        }
        let drawn = self.prepare_with(device, queue, &mut fonts, layer, canvas, out, warnings);
        self.own = Some(fonts);
        drawn
    }

    /// `prepare` with the compositor's fonts, which it refreshes once a frame (`fonts_changed`).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_with(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fonts: &mut SharedFonts,
        layer: &TextLayer,
        canvas: (u32, u32),
        out: (u32, u32),
        warnings: &mut Vec<String>,
    ) -> Option<(wgpu::Texture, ImageQuad, f64)> {
        if layer.text.is_empty() {
            return None;
        }
        let drawn = self.prepare_text(device, queue, fonts, layer, canvas, out, warnings);
        // A text drawn this frame says what its layout noted: a stand-in font, CJK without its Noto Sans.
        if drawn.is_some()
            && let Some(held) = self.drawn.get(&slot(layer))
        {
            self.said.say(held.layout.notes, warnings);
        }
        drawn
    }

    /// The scan finished, or a font was registered or removed (`SharedFonts::refresh`): every text is drawn again with
    /// the new fonts, and builds asked before are dropped when they land (counted until then).
    pub fn fonts_changed(&mut self) {
        self.builder.fonts_changed();
        self.drawn.clear();
        self.refused.clear();
        for asked in &mut self.in_flight {
            asked.dropped = true;
        }
        self.epoch += 1;
    }

    /// `prepare`'s work for a text with words, once the fonts are current.
    #[allow(clippy::too_many_arguments)]
    fn prepare_text(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fonts: &mut SharedFonts,
        layer: &TextLayer,
        canvas: (u32, u32),
        out: (u32, u32),
        warnings: &mut Vec<String>,
    ) -> Option<(wgpu::Texture, ImageQuad, f64)> {
        let slot = slot(layer);
        let geometry = Geometry::of(layer, canvas, out);
        let whole = geometry.sizes(geometry.up);
        let now = std::time::Instant::now();
        if self
            .refused
            .get(&slot)
            .is_some_and(|r| r.whole == whole && r.text == layer.text && now < r.retry_at)
        {
            // Still left out: said on every frame it is missing from.
            self.say_softer(warnings);
            return None;
        }
        self.refused.remove(&slot);
        // Asked ahead and not landed yet: waited for rather than built twice.
        self.wait_for(&slot, whole, layer);
        // The room this frame's other texts leave: textures kept for texts not in it give theirs up (`make_room`).
        // Builds asked ahead aren't counted against it: this text is on screen now and
        // they are for titles still to come, so they never soften it nor leave it out. They land within milliseconds
        // and are kept only if they fit then (`install`), so what is held passes the ceiling by at most their estimates,
        // and only until they land.
        let frame = self.frame;
        let others: u64 = self
            .drawn
            .iter()
            .filter(|(id, d)| **id != slot && d.seen == frame)
            .map(|(_, drawn)| drawn.bytes)
            .sum();
        let room = self
            .ceiling
            .saturating_sub(others.saturating_add(self.reserved));
        let open = self.ceiling.saturating_sub(self.reserved);
        let held = self
            .drawn
            .get(&slot)
            .filter(|drawn| drawn.whole == whole && drawn.key.text == layer.text)
            .map(|drawn| (drawn.down, drawn.retry_at));
        // A softened text tries a sharper layout at most once a second, climbing the ladder a new text climbs
        // (`Try::sharper`): full resolution while its box fits the ceiling (its ink checked against the room),
        // then the largest its estimate fits, never as soft as it is. So it is softer only while its ink doesn't fit.
        if let Some((down, retry_at)) = held
            && down < 1.0
            && retry_at.is_none_or(|at| now >= at)
        {
            let known = self.drawn.get(&slot).and_then(|d| d.too_big);
            let mut sharper = Try::sharper(&geometry, down, room, open, known);
            let mut first = true;
            while let Some(t) = sharper {
                match self.attempt(device, queue, fonts, layer, &geometry, t.down, room) {
                    Attempt::Drawn(texture, quad, scale) => {
                        if t.down < 1.0 {
                            self.say_softer(warnings);
                        }
                        return Some((texture, quad, scale));
                    }
                    // Its sharper layout is kept for when it shows.
                    Attempt::Hidden => return None,
                    // Its sharpest try that didn't fit is remembered (or one as sharp, at this room).
                    Attempt::TooBig(bytes) => {
                        if std::mem::take(&mut first)
                            && known.is_none_or(|(at, _)| t.down >= at)
                            && let Some(drawn) = self.drawn.get_mut(&slot)
                        {
                            drawn.too_big = Some((t.down, bytes));
                        }
                        sharper = t.next(&geometry, room).filter(|n| n.down > down);
                    }
                }
            }
            if let Some(drawn) = self.drawn.get_mut(&slot) {
                drawn.retry_at = Some(now + RETRY_AFTER);
            }
        }
        // Its held resolution (no layout while nothing changes). A new text climbs down the ladder a title built ahead
        // climbs too (`Try`): full resolution first when its estimate (its whole box) fits the ceiling,
        // as its texture is cut to its ink, which often fits where the box doesn't; then (one
        // layout more) the largest resolution its estimate fits, where any new text whose estimate is past the ceiling
        // starts; then softer while its lines (or a moved window) make it bigger than its room, unless there is no room
        // at all.
        let mut t = match held {
            Some((down, _)) => Try {
                down,
                whole_first: false,
            },
            None => match Try::first(&geometry, room, open) {
                Some(t) => t,
                None => return self.refuse(layer, whole, now, warnings),
            },
        };
        loop {
            match self.attempt(device, queue, fonts, layer, &geometry, t.down, room) {
                Attempt::Drawn(texture, quad, scale) => {
                    if t.down < 1.0 {
                        self.say_softer(warnings);
                    }
                    return Some((texture, quad, scale));
                }
                Attempt::Hidden => return None,
                Attempt::TooBig(_) => match t.next(&geometry, room) {
                    Some(next) => t = next,
                    None => return self.refuse(layer, whole, now, warnings),
                },
            }
        }
    }

    /// Says once a frame that texts are drawn softer, or left out, for the ceiling.
    fn say_softer(&mut self, warnings: &mut Vec<String>) {
        if !self.softer_said {
            self.softer_said = true;
            warnings.push(TEXT_OVER_BUDGET.into());
        }
    }

    /// `layer` doesn't fit even at its softest: not drawn, and not tried again for [`RETRY_AFTER`] unless it changes.
    /// Always said: an export stops on it, so a text is never silently missing.
    fn refuse(
        &mut self,
        layer: &TextLayer,
        whole: (u32, (u32, u32)),
        now: std::time::Instant,
        warnings: &mut Vec<String>,
    ) -> Option<(wgpu::Texture, ImageQuad, f64)> {
        self.say_softer(warnings);
        let slot = slot(layer);
        self.drawn.remove(&slot);
        self.refused.insert(
            slot,
            Refused {
                whole,
                text: layer.text.clone(),
                retry_at: now + RETRY_AFTER,
                seen: self.frame,
            },
        );
        None
    }

    /// `layer` at `down` of its full resolution: its held texture when its layout and window still serve, else laid
    /// out (reused when only the window moved) and drawn, when it fits in `room`.
    #[allow(clippy::too_many_arguments)]
    fn attempt(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fonts: &mut SharedFonts,
        layer: &TextLayer,
        geometry: &Geometry,
        down: f64,
        room: u64,
    ) -> Attempt {
        let slot = slot(layer);
        let scale = geometry.up * down;
        let (font_px, box_px) = geometry.sizes(scale);
        // Its layout and texture serve while the text, style and sizes are the same.
        let held = self
            .drawn
            .get(&slot)
            .filter(|drawn| drawn.key.matches(layer, font_px, box_px))
            .map(|drawn| (drawn.layout, drawn.drawn.clone()));
        let layout = match &held {
            Some((layout, _)) => *layout,
            None => {
                let key = geometry.key(layer, font_px, box_px);
                let (stroke, margin) = (geometry.stroke * scale, geometry.margin * scale);
                self.builder.begin(device, queue);
                self.builder
                    .lay_out(fonts, &key, stroke as f32, margin as f32)
            }
        };
        let k = layer.scale / scale;
        let frame = self.frame;
        let Some(windows) = shown(&layout, &geometry.place(scale)) else {
            // Nothing of it shows: its layout is kept for the next frame, its texture goes as before.
            match self.drawn.get_mut(&slot) {
                Some(drawn) if held.is_some() => {
                    drawn.drawn = None;
                    drawn.bytes = 0;
                    drawn.frame = frame;
                    drawn.seen = frame;
                    drawn.ahead = false;
                }
                _ => {
                    self.drawn.insert(
                        slot.clone(),
                        Drawn {
                            key: geometry.key(layer, font_px, box_px),
                            layout,
                            drawn: None,
                            frame,
                            bytes: 0,
                            down,
                            whole: geometry.sizes(geometry.up),
                            retry_at: None,
                            too_big: None,
                            seen: frame,
                            ahead: false,
                        },
                    );
                }
            }
            return Attempt::Hidden;
        };
        let kept = held
            .and_then(|(_, drawn)| drawn)
            .and_then(|(window, texture)| {
                (window.contains(windows.visible) || window == windows.cut)
                    .then_some((window, texture))
            });
        let (window, texture) = match kept {
            Some((window, texture)) => {
                let drawn = self.drawn.get_mut(&slot).expect("held");
                drawn.frame = frame;
                drawn.seen = frame;
                drawn.ahead = false;
                (window, texture)
            }
            None => {
                let mipmapped = scale > 1.0;
                let bytes = windows.cut.bytes(mipmapped);
                if bytes > room {
                    return Attempt::TooBig(bytes);
                }
                let key = geometry.key(layer, font_px, box_px);
                // The old texture goes first, so it never counts with the new one; textures kept for texts not in this
                // frame make room if it needs theirs (builds still to land aren't counted against a text on screen).
                self.drawn.remove(&slot);
                self.make_room(&slot, bytes, 0);
                self.builder.begin(device, queue);
                let texture =
                    self.builder
                        .draw(device, queue, fonts, &key, &layout, windows.cut, mipmapped);
                self.drawn.insert(
                    slot.clone(),
                    Drawn {
                        key,
                        layout,
                        drawn: Some((windows.cut, texture.clone())),
                        frame,
                        bytes,
                        down,
                        whole: geometry.sizes(geometry.up),
                        // A softened text keeps its resolution a second before it tries a sharper one.
                        retry_at: (down < 1.0).then(|| std::time::Instant::now() + RETRY_AFTER),
                        too_big: None,
                        seen: frame,
                        ahead: false,
                    },
                );
                self.note_peak();
                (windows.cut, texture)
            }
        };
        // The window's centre, from the layout's centre: turned and scaled as the quad turns and scales it.
        let (sin, cos) = (layer.rotation % 360.0).to_radians().sin_cos();
        let off_x =
            (f64::from(window.x) + f64::from(window.w) / 2.0 - f64::from(layout.full.0) / 2.0) * k;
        let off_y =
            (f64::from(window.y) + f64::from(window.h) / 2.0 - f64::from(layout.full.1) / 2.0) * k;
        let quad = ImageQuad {
            center: XY::new(
                geometry.centre.0 + cos * off_x - sin * off_y,
                geometry.centre.1 + sin * off_x + cos * off_y,
            ),
            size: XY::new(f64::from(window.w) * k, f64::from(window.h) * k),
            rotation_deg: layer.rotation,
            opacity: layer.opacity,
            rounding: 0.0,
            source_rotation: 0,
            mirror: false,
            blend: layer.blend,
        };
        Attempt::Drawn(texture, quad, scale)
    }
}
