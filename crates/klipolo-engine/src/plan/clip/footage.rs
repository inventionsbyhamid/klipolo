//! A main clip's footage at an instant: the screen, the camera or the imported video, cut by the clip's crop and framed
//! for the format (its effective framing: `klipolo_model::framing::effective_framing`), and a recording's camera bubble
//! or split pane. The crop is the clip's in every format: Fit shows what it leaves whole, Fill covers its area from
//! it, and the focus and the follow-crop move inside it. A camera bubble keeps its whole picture. With the blurred video
//! background, the main footage covers the frame again under itself, blurred and dimmed, wherever it leaves some of the
//! frame uncovered (`backdrop`). A screen whose framing is Fill with `follow` crops where its take's follow path is at that instant:
//! the path built from the raw cursor for that crop (`follow_crop_for`), found in the context's snapshot (`Follows`),
//! or the Fill crop at its focus until it is built (preview only: export loads it first). Follow on a Fit framing is
//! ignored: it follows the Fill crop.

use std::sync::Arc;

use klipolo_model::crop::{Crop, within};
use klipolo_model::framing::effective_framing;
use klipolo_model::layout::BoxN;
use klipolo_model::motion::{Blend, sample};
use klipolo_model::project::{
    Asset, AssetKind, BackgroundKind, FormatKind, Project, Recording, RecordingSource,
};
use klipolo_model::time::Us;
use klipolo_model::timeline::{CameraMode, Clip, ClipSource, Framing, FramingMode};

use super::super::effects::{self, Clock};
use super::super::framing::{
    self, FULL, area_aspect, as_shown, aspect_of, camera_rect_for, content_box, cover_crop,
    fit_rect, follow_crop_in, follow_crop_key, split_panes,
};
use super::super::strings::Table;
use super::super::takes::Follows;
use super::super::{Layer, MediaMap, PlanContext, SourceRef, VideoLayer, VideoRole};
use super::source_at;
use crate::registry::Registry;

const CENTRE_FILL: Framing = Framing {
    mode: FramingMode::Fill,
    focus_x: 0.5,
    focus_y: 0.5,
    follow: None,
};

/// How blurred the footage behind a clip is: the built-in blur's largest radius, in 1080p pixels.
const BACKDROP_BLUR_PX: f64 = 64.0;
/// How bright it shows over the black under it.
const BACKDROP_OPACITY: f64 = 0.6;

/// The main footage and, for recordings, the camera.
#[allow(clippy::too_many_arguments)]
pub(super) fn main_layers(
    p: &Project,
    (media, follows): (&MediaMap, &Follows),
    registry: &Registry,
    clip: &Clip,
    source_us: Us,
    format: FormatKind,
    s: &mut Table,
    out: &mut Vec<Layer>,
) {
    let content = content_box(p.background.padding, format);
    let video = |id: &str| p.asset(id).filter(|a| a.kind == AssetKind::Video);
    let main_id = s.str(&clip.id);
    let crop = clip.crop.unwrap_or_default();
    match &clip.source {
        ClipSource::Asset { id } => {
            if let Some(asset) = video(id) {
                let framing = effective_framing(p, clip, format);
                let main = framed(
                    VideoRole::Main,
                    main_id,
                    source_at(asset, source_us, media, registry, s),
                    content,
                    (framing, crop),
                    format,
                    false,
                );
                backdrop(p, registry, (clip, &main, content), format, s, out);
                out.push(Layer::Video(main));
            }
        }
        ClipSource::Recording { id } => {
            let Some(rec) = p.recording(id) else { return };
            // Each video source's file and its own time at this instant: across a gap between runs, or before or after
            // them, it holds its nearest frame.
            let camera = footage(p, rec.camera(), source_us);
            let Some((screen, screen_us)) = footage(p, rec.screen(), source_us) else {
                // Camera-only recording: the camera is the main footage.
                if let Some((camera, camera_us)) = footage(p, rec.primary_source(), source_us) {
                    let mirror = clip.camera.as_ref().is_some_and(|c| c.mirror);
                    let framing = effective_framing(p, clip, format);
                    let main = framed(
                        VideoRole::Main,
                        main_id,
                        source_at(camera, camera_us, media, registry, s),
                        content,
                        (framing, crop),
                        format,
                        mirror,
                    );
                    backdrop(p, registry, (clip, &main, content), format, s, out);
                    out.push(Layer::Video(main));
                }
                return;
            };
            // The layout depends only on the camera mode and whether the recording has a camera, never on whether
            // its footage covers this instant: outside it the camera holds its nearest frame (`run_or_nearest`), so
            // the split and the bubble don't jump at the edges of the camera recording.
            let shown = match (camera, &clip.camera) {
                (Some((asset, camera_us)), Some(layout)) => camera_rect_for(
                    layout,
                    p.background.padding,
                    p.settings.primary_format,
                    format,
                )
                .map(|rect| (asset, camera_us, layout, rect)),
                _ => None,
            };
            let split = shown.as_ref().is_some_and(|(_, _, layout, _)| {
                layout.mode_for(format) == CameraMode::SplitBottom
            });
            // The screen takes the clip's framing for this format (fit by default) in its area: the split's top pane,
            // else the whole content box.
            let area = if split {
                split_panes(content).0
            } else {
                content
            };
            let framing = effective_framing(p, clip, format);
            let mut main = framed(
                VideoRole::Main,
                main_id,
                source_at(screen, screen_us, media, registry, s),
                area,
                (framing, crop),
                format,
                false,
            );
            if let (FramingMode::Fill, Some(follow)) = (framing.mode, framing.follow) {
                let base = (main.crop, crop.kept());
                main.crop = followed(follows, rec, base, follow.closer, source_us);
            }
            backdrop(p, registry, (clip, &main, area), format, s, out);
            out.push(Layer::Video(main));
            if let Some((asset, camera_us, layout, rect)) = shown {
                // The bubble's keys are in the recording's source time (the clip's, not the camera file's), so cuts
                // and ripples keep them on the content; the split ignores motion.
                let keyed = layout
                    .bubble
                    .motion
                    .as_ref()
                    .filter(|_| !split)
                    .map(|m| sample(m, source_us, Us::MAX));
                let rect = keyed.map_or(rect, |k| {
                    let (w, h) = (rect.w * k.scale, rect.h * k.scale);
                    BoxN {
                        x: rect.x + k.dx + (rect.w - w) / 2.0,
                        y: rect.y + k.dy + (rect.h - h) / 2.0,
                        w,
                        h,
                    }
                });
                let mut cam = framed(
                    VideoRole::Camera,
                    s.camera(&clip.id),
                    source_at(asset, camera_us, media, registry, s),
                    rect,
                    (CENTRE_FILL, Crop::NONE),
                    format,
                    layout.mirror,
                );
                if !split {
                    cam.radius = framing::bubble_radius(layout.shape);
                    cam.border = layout.border;
                    cam.opacity = layout.bubble.opacity;
                    // The bubble turns by its own rotation plus its keys' (the composite draws it in the same pass).
                    cam.rotation = layout.bubble.rotation;
                    if let Some(k) = keyed {
                        cam.rotation += k.rotation;
                        cam.opacity *= k.opacity;
                    }
                }
                out.push(Layer::Video(cam));
            }
        }
    }
}

/// With the blurred video background (`BackgroundKind::Blur`), `main` (`clip`'s footage, framed in `area`) again under
/// it, onto `out`: covering the frame from the middle of what the clip's crop leaves, mirrored as `main` is, blurred
/// and dimmed over the black under it. Only where `main` leaves some of the frame uncovered: the background's padding,
/// or Fit's bars in `area` (a split's camera pane covers the rest). The same frame of the same file as `main`, so the
/// decoder reads it once; `clip_layers` gives it the clip's look, never its effects or the zoom.
fn backdrop(
    p: &Project,
    registry: &Registry,
    (clip, main, area): (&Clip, &VideoLayer, BoxN),
    format: FormatKind,
    s: &mut Table,
    out: &mut Vec<Layer>,
) {
    if p.background.kind != BackgroundKind::Blur
        || (p.background.padding <= 0.0 && covers(main.dest, area))
    {
        return;
    }
    let crop = clip.crop.unwrap_or_default();
    let frame = area_aspect(FULL, format);
    let aspect = crop.aspect_of(aspect_of(main.source.width, main.source.height, frame));
    let shown = within(crop.kept(), cover_crop(aspect, frame, 0.5, 0.5));
    let id = s.backdrop(&clip.id);
    // A still blur: its pass reads no time. The effect stage samples past the frame's edge as the edge, so the blur
    // reaches the edges at full strength.
    let clock = Clock {
        local_us: 0,
        duration_us: 0,
        fps: p.settings.fps,
    };
    let blur = effects::blur_pass(registry, &id, BACKDROP_BLUR_PX, clock);
    out.push(Layer::Video(VideoLayer {
        role: VideoRole::Backdrop,
        id,
        source: main.source.clone(),
        dest: FULL,
        crop: as_shown(main.mirror, shown),
        rotation: 0.0,
        opacity: BACKDROP_OPACITY,
        radius: 0.0,
        mirror: main.mirror,
        border: false,
        look: None,
        effects: blur.into_iter().collect(),
        pad_px: 0.0,
        blend: Blend::Normal,
        reach: 1.0,
    }));
}

/// Whether `rect` covers all of `area` (to a hair: framing's floating point).
fn covers(rect: BoxN, area: BoxN) -> bool {
    const EPS: f64 = 1e-6;
    rect.x <= area.x + EPS
        && rect.y <= area.y + EPS
        && rect.x + rect.w >= area.x + area.w - EPS
        && rect.y + rect.h >= area.y + area.h - EPS
}

/// The y share where a portrait split's two halves meet at `at_us` (the screen above, the camera below), or None when
/// the main clip there isn't split in `format`: the same rects `frame_plan` draws them in (the padded area's middle).
/// As there, the layout depends on the camera mode and whether the recording has a camera, never on whether its
/// footage covers this instant. A caption style placed at the seam (Arcade) sits on it.
pub fn split_seam(ctx: &PlanContext, at_us: Us, format: FormatKind) -> Option<f64> {
    if format != FormatKind::Portrait {
        return None;
    }
    let p = &*ctx.project;
    let (i, _) = ctx.clip_at(at_us)?;
    let clip = p.timeline.main.get(i)?;
    let ClipSource::Recording { id } = &clip.source else {
        return None;
    };
    let rec = p.recording(id)?;
    let layout = clip.camera.as_ref()?;
    let split = rec.screen().is_some()
        && rec.camera().is_some()
        && layout.mode_for(format) == CameraMode::SplitBottom;
    split.then(|| split_panes(content_box(p.background.padding, format)).1.y)
}

/// A recording source's video file and its own time at recording time `rec_us` (`run_or_nearest`).
fn footage<'a>(
    p: &'a Project,
    source: Option<&RecordingSource>,
    rec_us: Us,
) -> Option<(&'a Asset, Us)> {
    let (run, at) = source?.run_or_nearest(rec_us)?;
    let asset = p.asset(&run.asset_id)?;
    (asset.kind == AssetKind::Video).then_some((asset, at))
}

/// `base` (the screen's Fill crop at its focus) where `rec`'s follow path for it is at recording time `source_us` (the
/// cursor rides the screen's frames, so the screen's nudge moves it too), kept inside `kept` (what the clip's crop
/// leaves); the path's first centre before its first sample, and `base`'s own centre until the path is built.
/// O(log paths), no allocation.
fn followed(
    follows: &Follows,
    rec: &Recording,
    (base, kept): (BoxN, BoxN),
    closer: f64,
    source_us: Us,
) -> BoxN {
    let nudge = rec.screen().map_or(0, |s| s.nudge_us);
    let centre = (rec.cursor_path())
        .and_then(|path| follows.get(path, follow_crop_key(base, closer)))
        .and_then(|f| f.centre_at(source_us.saturating_sub(nudge)))
        .unwrap_or((base.x + base.w / 2.0, base.y + base.h / 2.0));
    follow_crop_in(kept, base, centre, closer)
}

/// Which part of `clip`'s main footage its Fill shows from the centre in `format`, as `main_layers` frames it, in the
/// coordinates of what the clip's crop leaves of it (0 to 1 on both axes): narrower than that where Fill crops across,
/// shorter where it crops down, all of it where it fits its area exactly. None when the clip has no video to draw. The
/// clip card offers the Fill focus only the way Fill crops (`cover_crop` moves the crop along that axis alone).
pub fn fill_crop(p: &Project, clip: &Clip, format: FormatKind) -> Option<BoxN> {
    let content = content_box(p.background.padding, format);
    let (footage, area) = match &clip.source {
        ClipSource::Asset { id } => (p.asset(id)?, content),
        ClipSource::Recording { id } => {
            let rec = p.recording(id)?;
            match first_video(p, rec.screen()) {
                Some(screen) => (screen, screen_area(p, clip, rec, format)),
                // Camera-only recording: the camera is the main footage, in the whole content box.
                None => (first_video(p, rec.primary_source())?, content),
            }
        }
    };
    if footage.kind != AssetKind::Video {
        return None;
    }
    let target = area_aspect(area, format);
    let crop = clip.crop.unwrap_or_default();
    let aspect = crop.aspect_of(aspect_of(footage.width, footage.height, target));
    Some(cover_crop(aspect, target, 0.5, 0.5))
}

/// The video file of a recording source's first run, if it has one.
fn first_video<'a>(p: &'a Project, source: Option<&RecordingSource>) -> Option<&'a Asset> {
    let run = source?.runs.first()?;
    p.asset(&run.asset_id)
        .filter(|a| a.kind == AssetKind::Video)
}

/// Where `clip`'s screen (playing `rec`) is framed in `format`, as `main_layers` frames it: the split's top pane while
/// a camera with video is split below it, else the content box.
fn screen_area(p: &Project, clip: &Clip, rec: &Recording, format: FormatKind) -> BoxN {
    let camera = (rec.camera().into_iter().flat_map(|c| &c.runs)).any(|r| {
        p.asset(&r.asset_id)
            .is_some_and(|a| a.kind == AssetKind::Video)
    });
    let split = camera
        && (clip.camera.as_ref()).is_some_and(|l| l.mode_for(format) == CameraMode::SplitBottom);
    let content = content_box(p.background.padding, format);
    if split {
        split_panes(content).0
    } else {
        content
    }
}

/// The follow path `clip` (playing `rec`) crops with in `format`, as its crop key (`follow_crop_key`), when its
/// effective framing follows: what the take's loader builds (`media::cursor::wanted`) and a frame finds. The crop is
/// the screen's Fill crop in its area (the split's top pane, else the content box) of what the clip's crop leaves, as
/// `main_layers` frames it.
pub fn follow_crop_for(
    p: &Project,
    clip: &Clip,
    rec: &Recording,
    format: FormatKind,
) -> Option<(u16, u16, u16)> {
    let framing = effective_framing(p, clip, format);
    let follow = framing
        .follow
        .filter(|_| framing.mode == FramingMode::Fill)?;
    let screen = first_video(p, rec.screen())?;
    let target = area_aspect(screen_area(p, clip, rec, format), format);
    let crop = clip.crop.unwrap_or_default();
    let base = within(
        crop.kept(),
        cover_crop(
            crop.aspect_of(aspect_of(screen.width, screen.height, target)),
            target,
            0.5,
            0.5,
        ),
    );
    Some(follow_crop_key(base, follow.closer))
}

/// `source` cut by `crop` and framed in `area` by `framing`: Fit shows what the crop leaves whole, Fill covers `area`
/// from it at the framing's focus. The crop and the focus are on the picture as shown, so on mirrored footage "left"
/// cuts, and "right" keeps, the side the person sees.
fn framed(
    role: VideoRole,
    id: Arc<str>,
    source: SourceRef,
    area: BoxN,
    (framing, crop): (Framing, Crop),
    format: FormatKind,
    mirror: bool,
) -> VideoLayer {
    let target = area_aspect(area, format);
    let kept = crop.kept();
    let aspect = crop.aspect_of(aspect_of(source.width, source.height, target));
    let (dest, shown) = match framing.mode {
        FramingMode::Fit => (fit_rect(area, aspect, format), kept),
        FramingMode::Fill => (
            area,
            within(
                kept,
                cover_crop(aspect, target, framing.focus_x, framing.focus_y),
            ),
        ),
    };
    let crop = as_shown(mirror, shown);
    VideoLayer {
        role,
        id,
        source,
        dest,
        crop,
        rotation: 0.0,
        opacity: 1.0,
        radius: 0.0,
        mirror,
        border: false,
        look: None,
        effects: Vec::new(),
        pad_px: 0.0,
        blend: Blend::Normal,
        reach: 1.0,
    }
}
