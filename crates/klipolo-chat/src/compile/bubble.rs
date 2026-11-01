//! `adjustCamera`: where the camera bubble sits, its size, shape, border and mirror, on the clips or on a camera that
//! is its own layer.
//! - The built-in bubbles move, resize and restyle. Their own box is relative to the primary format; with another
//!   format named, that format keeps a box of its own (`Layout::overrides`), which keeps the corner it sits in. With no
//!   format named, every format moves: the own box, and each listed format's box of its own (one a new main format
//!   gave the old one), each from where it is. A change of shape, border or mirror alone moves no box.
//! - A camera that is its own layer moves and resizes as that layer, takes the shape as its corners and the mirror as
//!   its own; it has no border, which a note says.
//! - A camera that is its clip's footage takes only the mirror; a bubble change for it is noted, and refused when no
//!   other camera takes it.
//!
//! Sizes are squares on the format's canvas, the same bubble in every format: small, medium and large are 0.14, 0.2 and
//! 0.28 of a 16:9 frame's width (medium is the bubble a take starts with). A step scales both sides by 1.25 or 0.8
//! around the bubble's corner, never past the frame. A place is one of the frame's nine, 4 % from the edges it touches.

use klipolo_model::Project;
use klipolo_model::layout::{Anchor, BoxN, Layout, anchor_for_rect, resolve_rect};
use klipolo_model::ops::{ClipPatch, ItemPatch, Op};
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::{CameraShape, ItemContent, ROUNDED_CAMERA_RADIUS};

use super::cameras::{bubbles, clip_of, find, no_bubble};
use super::recorded::{Noted, has_format, named, noted};
use super::{Compiler, Edit, Refused, label, to_model};
use crate::tools::{BubbleChange, BubbleSize, SizeStep};

/// Said when a border is asked of a camera that is its own layer.
pub const LAYER_NO_BORDER: &str = "A camera that is its own layer has no border.";
/// The margin from the edges a place touches, as a share of the frame.
const MARGIN: f64 = 0.04;

pub(super) fn adjust(
    c: &Compiler,
    work: &Project,
    clips: Option<&[String]>,
    a: &BubbleChange,
) -> Result<Noted, Refused> {
    const TOOL: &str = "adjustCamera";
    if a.is_empty() {
        return Err(to_model(format!(
            "{TOOL}: say what changes: position, size, sizeStep, shape, border or mirror."
        )));
    }
    if let Some(f) = a.format {
        has_format(work, f)?;
    }
    let formats = (
        work.settings.primary_format,
        work.settings.formats.as_slice(),
    );
    let named = named(c, work, TOOL, clips)?;
    let cams = find(work, TOOL, named.as_ref())?;
    let cams = if a.mirror.is_some() {
        cams
    } else {
        bubbles(work, cams)?
    };
    let mut ops = Vec::new();
    for id in &cams.footage {
        let (Some(cam), Some(mirror)) = (
            clip_of(work, id).and_then(|clip| clip.camera.as_ref()),
            a.mirror,
        ) else {
            continue;
        };
        if cam.mirror != mirror {
            let mut now = cam.clone();
            now.mirror = mirror;
            ops.push(update_clip(id, now));
        }
    }
    for id in &cams.on_clips {
        let Some(cam) = clip_of(work, id).and_then(|clip| clip.camera.as_ref()) else {
            continue;
        };
        let mut now = cam.clone();
        now.bubble = placed(&cam.bubble, a, formats);
        if let Some(shape) = a.shape {
            now.shape = shape;
        }
        if let Some(border) = a.border {
            now.border = border;
        }
        if let Some(mirror) = a.mirror {
            now.mirror = mirror;
        }
        if now != *cam {
            ops.push(update_clip(id, now));
        }
    }
    for layer in cams.layers() {
        let Some(item) = (work.timeline.tracks.iter())
            .flat_map(|t| &t.items)
            .find(|i| i.id == layer)
        else {
            continue;
        };
        let mut layout = placed(&item.layout, a, formats);
        if let Some(shape) = a.shape {
            layout.radius = match shape {
                CameraShape::Circle => 0.5,
                CameraShape::Rounded if layout.radius > 0.0 && layout.radius < 0.5 => layout.radius,
                CameraShape::Rounded => ROUNDED_CAMERA_RADIUS,
                CameraShape::Square => 0.0,
            };
        }
        let mut content = item.content.clone();
        if let (ItemContent::Video { mirror, .. }, Some(m)) = (&mut content, a.mirror) {
            *mirror = m;
        }
        let patch = ItemPatch {
            layout: (layout != item.layout).then_some(layout),
            content: (content != item.content).then_some(content),
            ..ItemPatch::default()
        };
        if patch.layout.is_some() || patch.content.is_some() {
            ops.push(Op::UpdateItem { id: layer, patch });
        }
    }
    let mut notes = Vec::new();
    // A bubble's settings have nothing to change on a camera that is its clip's footage: said once.
    let bubble_only = a.position.is_some()
        || a.size.is_some()
        || a.size_step.is_some()
        || a.shape.is_some()
        || a.border.is_some();
    if let Some(clip) = cams.footage.first().filter(|_| bubble_only) {
        notes.push(no_bubble(work, clip));
    }
    if !cams.separate.is_empty() && a.border.is_some() {
        notes.push(LAYER_NO_BORDER.to_owned());
    }
    noted(
        Edit {
            ops,
            label: label(TOOL, Vec::new()),
        },
        notes,
    )
}

fn update_clip(id: &str, camera: klipolo_model::timeline::CameraLayout) -> Op {
    Op::UpdateClip {
        id: id.to_owned(),
        patch: ClipPatch {
            camera: Some(camera),
            ..ClipPatch::default()
        },
    }
}

/// `layout` moved and sized as asked, in the video's formats (`primary` first): with a format named, that format's box
/// alone (the own box for the primary, else a box of its own); with none, the own box and each listed format's box of
/// its own, so whichever format the person looks at, the bubble moves. Without a place or a size, as it was.
fn placed(
    layout: &Layout,
    a: &BubbleChange,
    (primary, formats): (FormatKind, &[FormatKind]),
) -> Layout {
    if a.position.is_none() && a.size.is_none() && a.size_step.is_none() {
        return layout.clone();
    }
    match a.format {
        Some(f) if f != primary => moved(layout, a, primary, Some(f)),
        Some(_) => moved(layout, a, primary, None),
        None => {
            let own =
                (formats.iter()).filter(|f| **f != primary && layout.overrides.contains_key(f));
            own.fold(moved(layout, a, primary, None), |out, f| {
                moved(&out, a, primary, Some(*f))
            })
        }
    }
}

/// `layout` moved and sized as asked: its own box (relative to the primary format), or with `only`, that format's.
fn moved(
    layout: &Layout,
    a: &BubbleChange,
    primary: FormatKind,
    only: Option<FormatKind>,
) -> Layout {
    let mut layout = layout.clone();
    // A format's own box keeps the corner it sits in (its anchor is where it is, not the primary box's).
    let (mut rect, mut anchor) = match only {
        Some(f) => {
            let rect = resolve_rect(&layout, primary, f);
            (rect, anchor_for_rect(&rect))
        }
        None => (layout.rect, layout.anchor),
    };
    if let Some(size) = a.size {
        let (w, h) = bubble_size(size, only.unwrap_or(primary));
        rect = place(anchor, w, h, MARGIN);
    }
    if let Some(step) = a.size_step {
        let f: f64 = match step {
            SizeStep::Bigger => 1.25,
            SizeStep::Smaller => 0.8,
        };
        // Both sides scale alike, so the bubble stays square: never past the frame, never under the smallest box.
        let f = (f.min(1.0 / rect.w.max(rect.h))).max(0.01 / rect.w.min(rect.h));
        rect = scale_box(rect, anchor, f);
    }
    if let Some(pos) = a.position {
        anchor = pos;
        rect = place(pos, rect.w, rect.h, MARGIN);
    }
    match only {
        Some(f) => {
            layout.overrides.insert(f, rect);
        }
        None => {
            layout.rect = rect;
            layout.anchor = anchor;
        }
    }
    layout
}

/// A square bubble on `format`'s canvas, as a box's (w, h): its side in pixels scales with the canvas's shorter side,
/// as `resolve_rect` carries sizes between formats, so each size is the same bubble in every format.
fn bubble_size(size: BubbleSize, format: FormatKind) -> (f64, f64) {
    let w = match size {
        BubbleSize::Small => 0.14,
        BubbleSize::Medium => 0.2,
        BubbleSize::Large => 0.28,
    };
    let (cw, ch) = format.size();
    let (cw, ch) = (f64::from(cw), f64::from(ch));
    let side = w * 1920.0 / 1080.0 * cw.min(ch);
    (side / cw, side / ch)
}

/// The point of the frame an anchor names: top left (0, 0) to bottom right (1, 1).
fn focus_of(anchor: Anchor) -> (f64, f64) {
    use Anchor::*;
    let x = match anchor {
        TopLeft | MiddleLeft | BottomLeft => 0.0,
        TopCenter | Center | BottomCenter => 0.5,
        TopRight | MiddleRight | BottomRight => 1.0,
    };
    let y = match anchor {
        TopLeft | TopCenter | TopRight => 0.0,
        MiddleLeft | Center | MiddleRight => 0.5,
        BottomLeft | BottomCenter | BottomRight => 1.0,
    };
    (x, y)
}

/// A `w`×`h` box at `anchor`, `margin` from the edges it touches.
fn place(anchor: Anchor, w: f64, h: f64, margin: f64) -> BoxN {
    let (w, h) = (w.clamp(0.01, 1.0), h.clamp(0.01, 1.0));
    let (fx, fy) = focus_of(anchor);
    let at = |f: f64, size: f64| match f {
        f if f < 0.25 => margin,
        f if f > 0.75 => 1.0 - margin - size,
        _ => (1.0 - size) / 2.0,
    };
    BoxN {
        x: at(fx, w).clamp(0.0, 1.0 - w),
        y: at(fy, h).clamp(0.0, 1.0 - h),
        w,
        h,
    }
}

/// The same box scaled by `f` around the point its anchor names (a bigger bubble stays in its corner).
fn scale_box(rect: BoxN, anchor: Anchor, f: f64) -> BoxN {
    let (fx, fy) = focus_of(anchor);
    let (px, py) = (rect.x + rect.w * fx, rect.y + rect.h * fy);
    let (w, h) = ((rect.w * f).clamp(0.01, 1.0), (rect.h * f).clamp(0.01, 1.0));
    BoxN {
        x: (px - w * fx).clamp(0.0, 1.0 - w),
        y: (py - h * fy).clamp(0.0, 1.0 - h),
        w,
        h,
    }
}
