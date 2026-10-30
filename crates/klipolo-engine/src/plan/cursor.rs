//! The cursor over one main clip at one instant: where the take's cursor lands on the
//! footage after zoom and framing, how big and how visible it is, its press, ripples and smear. A pure function of the
//! context (the project and the takes it loaded) and the source time, so preview, scrubbing and export plan the same
//! cursor: export loads the takes the preview loads (`media::cursor::CursorTracks`).
//!
//! **Nothing is drawn** where the frames already show the cursor (the recording's `capture`, the file's own `x` line,
//! and a heal gap's second half, where the footage holds the healed run's first frame), where the clip's style hides
//! it, before the take's first position, once it has faded out at rest, or more than one cursor height outside the
//! footage's box, unless a ripple is still running: then the layer holds the ripples alone (opacity 0), so they finish
//! where the click was, clipped to the view. After the last position the track holds it, which covers a silent
//! source's gap before its burn-in. Ripples and the press run on screen time (the clip's speed taken
//! out), and a shape mask on the footage masks the cursor too (`mask`).
//!
//! **Cost:** O(log n) in the take's positions, presses and shapes: a few binary searches, under the cache's and the
//! take's locks for microseconds, so a frame never holds, and never frees, a take or a path the loader lets go.
//! **No allocation:** the id is interned once per clip (`Table::cursor`); the take's key and its shapes'
//! paths are the cache's own shared strings. `effects` holds the press, ripples and smear; `place` maps the footage;
//! `shapes` lists the shapes an export range draws.

mod effects;
mod mask;
mod place;
mod shapes;

use std::sync::Arc;

use klipolo_cursor::facts::idle_opacity;
use klipolo_cursor::{ClickSource, CursorTrack};
use klipolo_model::cursor_style::{ClickEffect, CursorCapture, CursorLook};
use klipolo_model::project::{FormatKind, Recording, RecordingSource};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource};

use super::strings::Table;
use super::{CursorDraw, CursorLayer, Layer, PlanContext, Ripple, VideoLayer};
use crate::media::cursor::TakeCursor;
use place::Place;
pub use shapes::shapes_in;

/// A 180° shutter: the smear covers half a frame at the project's rate.
pub const SHUTTER: f64 = 0.5;
/// How long a ripple grows and fades.
pub const RIPPLE_US: Us = 450_000;
/// How wide a ripple grows, in cursor heights.
pub const RIPPLE_REACH: f64 = 2.2;
/// The cursor's size while the primary button is held.
pub const PRESS: f64 = 0.85;
/// The height in points of Klipolo's own looks (Dot and Arrow), also drawn for a shape whose image can't be used.
pub const ARROW_PTS: f64 = 22.0;
/// The longest smear, in cursor heights.
const BLUR_REACH: f64 = 4.0;

/// Selection id of a recording clip's cursor, shared with the UI.
pub fn cursor_layer_id(clip_id: &str) -> String {
    format!("cursor:{clip_id}")
}

/// Puts main clip `clip`'s cursor, when it plays a recording, right after its main footage `out[main]`: over the
/// footage, after its zoom, look and effects, and under the camera.
pub(super) fn put_over(
    ctx: &PlanContext,
    clip: &Clip,
    source_us: Us,
    format: FormatKind,
    s: &mut Table,
    out: &mut Vec<Layer>,
    main: usize,
) {
    let ClipSource::Recording { id } = &clip.source else {
        return;
    };
    let (Some(rec), Some(Layer::Video(footage))) = (ctx.project.recording(id), out.get(main))
    else {
        return;
    };
    if let Some(cursor) = cursor_layer(ctx, clip, rec, source_us, footage, format, s) {
        out.insert(main + 1, Layer::Cursor(cursor));
    }
}

/// The cursor over main clip `clip` (recording `rec`) at its source time `source_us`, drawn over `footage` (the main
/// footage's final crop and box, after zoom and framing) in `format`, or None: no take, hidden, burned in there, outside
/// the picture. O(log n), and no allocation once the clip's id is interned. Public for the allocation and speed tests.
pub fn cursor_layer(
    ctx: &PlanContext,
    clip: &Clip,
    rec: &Recording,
    source_us: Us,
    footage: &VideoLayer,
    format: FormatKind,
    s: &mut Table,
) -> Option<CursorLayer> {
    let cursor = rec.cursor.as_ref()?;
    let screen = rec.screen()?;
    // The cursor rides the screen's frames, so the screen's sync nudge moves it too.
    let rec_us = source_us.saturating_sub(screen.nudge_us);
    let style = ctx.project.cursor_for(clip).effective();
    if style.hidden
        || !cursor.capture.separate_at(rec_us)
        || healed_run_shown(cursor.capture, screen, source_us)
    {
        return None;
    }
    let place = Place::of(footage, format)?;
    let (ripple, press) = match style.click {
        ClickEffect::None => (false, false),
        ClickEffect::Ripple => (true, false),
        ClickEffect::Press => (false, true),
        ClickEffect::RippleAndPress => (true, true),
    };
    let fps = ctx.project.settings.fps;
    // Ripples and the press run on screen time, so they look the same at any clip speed.
    let speed = if clip.speed.is_finite() && clip.speed > 0.0 {
        clip.speed
    } else {
        1.0
    };
    ctx.cursors
        .with_take(&cursor.path, |key, take| {
            let track = &*take.track;
            if !track.separate_at(rec_us) {
                return None;
            }
            let rippling = ripple
                && track.clicks_at(rec_us) != ClickSource::None
                && effects::rippling(track, rec_us, speed);
            let hide = style.hide_when_still_us;
            let opacity = f64::from(idle_opacity(track, &take.still, rec_us, hide));
            // NaN or none: hidden, but a ripple still finishes where it was clicked.
            let opacity = if opacity > 0.0 { opacity } else { 0.0 };
            if opacity == 0.0 && !rippling {
                return None;
            }
            let (point, velocity) = take.with_drawn(style.smoothing, |path| {
                (path.position(track, rec_us), path.velocity(track, rec_us))
            });
            let (look, pts) = look(style.look, take, key, rec_us);
            let height = pts / area_height(track, rec_us, footage) * place.zoom() * style.size;
            let hotspot = place.point(point?);
            // Off the (zoomed) view the cursor isn't drawn; its ripples are, clipped to the view.
            let shown = opacity > 0.0 && place.near(hotspot, height);
            if !shown && !rippling {
                return None;
            }
            let (ripples, ripple_count) = if rippling {
                effects::ripples(track, rec_us, speed, height, style.click_color, &place)
            } else {
                ([Ripple::default(); 4], 0)
            };
            Some(CursorLayer {
                id: s.cursor(&clip.id),
                look,
                hotspot,
                height,
                opacity: if shown { opacity } else { 0.0 },
                press: if press {
                    effects::press(track, rec_us, speed)
                } else {
                    1.0
                },
                blur: effects::blur(velocity, clip.speed, fps, style.motion_blur, &place, height),
                clip: footage.dest,
                radius: footage.radius,
                mask: mask::of(&footage.effects),
                ripples,
                ripple_count,
            })
        })
        .flatten()
}

/// In a heal gap before a burn-in the footage holds its nearest frame (`run_or_nearest`, as the clip
/// reads it at `source_us`), which past the gap's middle is the healed run's first: its frames show the cursor, so the
/// held one isn't drawn there too. O(runs), only for a take burned in part-way.
fn healed_run_shown(capture: CursorCapture, screen: &RecordingSource, source_us: Us) -> bool {
    let CursorCapture::BurnedFrom { at_us } = capture else {
        return false;
    };
    screen
        .run_or_nearest(source_us)
        .is_some_and(|(run, _)| run.offset_us >= at_us)
}

/// What draws, and its height in points: the take's shape when its image is usable, else the Arrow.
fn look(look: CursorLook, take: &TakeCursor, key: &Arc<str>, t: Us) -> (CursorDraw, f64) {
    let shape = || {
        let def = take.track.shape_at(t)?;
        let file = take.shape_file(def.id)?.clone();
        let (w, h) = (f32::from(def.px.0), f32::from(def.px.1));
        let (hx, hy) = def.hotspot_px;
        let draw = CursorDraw::Shape {
            take: key.clone(),
            shape: def.id,
            file,
            hotspot: (hx / w, hy / h),
            aspect: w / h,
        };
        Some((draw, f64::from(def.pts)))
    };
    match look {
        CursorLook::Recorded => shape().unwrap_or((CursorDraw::Arrow, ARROW_PTS)),
        CursorLook::Dot => (CursorDraw::Dot, ARROW_PTS),
        CursorLook::Arrow => (CursorDraw::Arrow, ARROW_PTS),
    }
}

/// The captured area's height in points at `t`; where the file doesn't say (0 × 0), the screen's height in pixels.
fn area_height(track: &CursorTrack, t: Us, footage: &VideoLayer) -> f64 {
    let h = f64::from(track.area_at(t).1);
    if h > 0.0 && h.is_finite() {
        h
    } else {
        f64::from(footage.source.height.max(1))
    }
}
