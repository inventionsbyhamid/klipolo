//! Which of a screen stream's `Complete` frames changed only Klipolo's own windows: the camera
//! bubble's live picture and the HUD are left out of a display's picture, yet ScreenCaptureKit still delivers a frame
//! each time they change, at the camera's rate over a still screen. The rule is `common::our_damage`; this reads its
//! inputs from the frame (its dirty rectangles), Klipolo's windows (`own_windows`) and the cursor, and counts what it
//! found, logged when the stream ends (numbers only).
//!
//! Only a picture that is a whole display (or an app's windows on it) is looked at; a single window's picture never
//! shows Klipolo's windows, so its frames are always real.

use cidre::{cg, cm, sc};
use objc2::runtime::AnyObject;
use objc2_app_kit::NSEvent;
use objc2_foundation::NSValue;

use super::super::own_windows;
use super::feed::info_of;
use crate::platform::common::our_damage::{CURSOR_PT, Freshness, SLACK_PX, only_ours, rect_in};
use crate::platform::common::sck::{CgRect, cursor_in};

/// Frames whose details are logged, the first few that carry dirty rectangles.
const LOGGED: u32 = 3;

/// Where a whole display's picture is, in AppKit's global points: the display's frame and the captured area in it (the
/// whole display: an app's windows on it are pictured on the display too). The cursor's area, kept for this rule
/// when the recorded cursor became its own source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::platform::macos) struct PictureArea {
    display: CgRect,
    captured: CgRect,
}

impl PictureArea {
    /// Display `id`'s picture.
    pub(in crate::platform::macos) fn display(id: cg::DirectDisplayId) -> Self {
        let display = cocoa(id.bounds());
        Self {
            display,
            captured: CgRect {
                x: 0.0,
                y: 0.0,
                w: display.w,
                h: display.h,
            },
        }
    }
}

/// Core Graphics' global rectangle (top-left origin, y down) in AppKit's global points (bottom-left origin, y up).
fn cocoa(r: cg::Rect) -> CgRect {
    let main_h = cg::DirectDisplayId::main().bounds().size.height;
    CgRect {
        x: r.origin.x,
        y: main_h - (r.origin.y + r.size.height),
        w: r.size.width,
        h: r.size.height,
    }
}

/// One stream's look at its frames' damage.
pub(in crate::platform::macos) struct OwnDamage {
    /// The picture's place on the display; None for a single window's picture (never looked at).
    area: Option<PictureArea>,
    /// The cursor is drawn in the picture.
    shows_cursor: bool,
    /// The frame's dirty rectangles, as picture fractions (kept, so a frame allocates nothing).
    dirty: Vec<CgRect>,
    /// Whether the file's picture is fresh (`our_damage::Freshness`): pauses, dropped frames, the cursor's place.
    fresh: Freshness,
    /// The cursor (AppKit's global points) for the frame last looked at, recorded when that frame is sent.
    cursor_now: Option<(f64, f64)>,
    counts: Counts,
}

#[derive(Debug, Default)]
struct Counts {
    looked: u32,
    no_attachment: u32,
    empty: u32,
    ours: u32,
    cursor_near: u32,
    cursor_moved: u32,
    /// Ours, yet encoded for a fresh picture (after a pause or a dropped real frame).
    refreshed: u32,
    skipped: u32,
    logged_ours: u32,
    logged_real: u32,
}

impl OwnDamage {
    pub(in crate::platform::macos) fn new(area: Option<PictureArea>, shows_cursor: bool) -> Self {
        Self {
            area,
            shows_cursor,
            dirty: Vec::with_capacity(16),
            fresh: Freshness::default(),
            cursor_now: None,
            counts: Counts::default(),
        }
    }

    /// Whether the cursor is drawn in the picture now: it can start mid-run, with the cursor's live switch.
    pub(in crate::platform::macos) fn shows_cursor(&mut self, on: bool) {
        self.shows_cursor = on;
    }

    /// A buffer came while the take is paused: the first frame after the resume is encoded.
    pub(in crate::platform::macos) fn paused(&mut self) {
        self.fresh.paused();
    }

    /// A real frame was handed to the encoder (`queued`), or dropped on its way there.
    pub(in crate::platform::macos) fn sent(&mut self, queued: bool) {
        self.fresh.sent(queued, self.cursor_now);
    }

    /// Whether this `Complete` frame is handled as idle: it changed only Klipolo's windows, and nothing needs it encoded
    /// (`our_damage::Freshness::skip`).
    pub(in crate::platform::macos) fn idle(
        &mut self,
        sample: &cm::SampleBuf,
        key_wanted: bool,
        after_blank: bool,
    ) -> bool {
        let Some(area) = self.area else {
            return false;
        };
        let global = self.shows_cursor.then(|| {
            let at = NSEvent::mouseLocation();
            (at.x, at.y)
        });
        self.cursor_now = global;
        let c = &mut self.counts;
        c.looked += 1;
        let attached = dirty_rects(sample, &mut self.dirty);
        if !attached {
            c.no_attachment += 1;
        } else if self.dirty.is_empty() {
            c.empty += 1;
        }
        let mut ours = [CgRect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        }; 2];
        let mut n = 0;
        for window in own_windows::shown().into_iter().flatten() {
            ours[n] = rect_in(window, area.display, area.captured);
            n += 1;
        }
        let cursor = global.map(|at| {
            let (x, y) = cursor_in(at, area.display, area.captured);
            (f64::from(x), f64::from(y))
        });
        let reach = (
            CURSOR_PT / area.captured.w.max(1.0),
            CURSOR_PT / area.captured.h.max(1.0),
        );
        let (w, h) = sample
            .image_buf()
            .map_or((1.0, 1.0), |i| (i.width() as f64, i.height() as f64));
        let slack = (SLACK_PX / w.max(1.0), SLACK_PX / h.max(1.0));
        let dirty = attached.then_some(&self.dirty[..]);
        let only = only_ours(dirty, &ours[..n], cursor, reach, slack);
        let idle = self.fresh.skip(only, key_wanted, after_blank, global);
        c.ours += u32::from(only);
        c.skipped += u32::from(idle);
        // Ours but for the cursor near one of Klipolo's windows, or for the cursor having moved (counted, so the device
        // check can tell them apart).
        let cursor_near =
            !only && cursor.is_some() && only_ours(dirty, &ours[..n], None, reach, slack);
        c.cursor_near += u32::from(cursor_near);
        c.cursor_moved += u32::from(only && self.fresh.cursor_moved(global));
        c.refreshed += u32::from(only && self.fresh.needs_fresh());
        // The first few frames over Klipolo's windows, ours or not: their numbers, for the device check.
        let logged = if only {
            &mut c.logged_ours
        } else {
            &mut c.logged_real
        };
        if attached && n > 0 && *logged < LOGGED {
            *logged += 1;
            let shown: Vec<_> = self.dirty.iter().take(3).collect();
            tracing::info!(
                rects = self.dirty.len(),
                ?shown,
                ours = ?&ours[..n],
                ?cursor,
                only,
                idle,
                "screen: a frame's dirty rectangles, as picture fractions"
            );
        }
        idle
    }
}

impl Drop for OwnDamage {
    fn drop(&mut self) {
        if self.area.is_some() {
            tracing::info!(counts = ?self.counts, "screen: frames that changed only Klipolo's windows");
        }
    }
}

/// The frame's dirty rectangles (`SCStreamFrameInfoDirtyRects`, in the frame's pixels) as fractions of the frame, into
/// `out`; false when the frame has no such attachment, or one that can't be read (a real change, never guessed).
fn dirty_rects(sample: &cm::SampleBuf, out: &mut Vec<CgRect>) -> bool {
    out.clear();
    let key = sc::FrameInfo::dirty_rects().as_cf();
    let Some(list) = info_of(sample)
        .and_then(|i| i.value(key))
        .and_then(|v| v.try_as_array())
    else {
        return false;
    };
    let Some(image) = sample.image_buf() else {
        return false;
    };
    let (w, h) = (image.width() as f64, image.height() as f64);
    if w <= 0.0 || h <= 0.0 {
        return false;
    }
    for item in list.iter() {
        let rect = match item.try_as_dictionary() {
            Some(d) => {
                let side = |name: &cidre::cf::String| {
                    (d.value(name))
                        .and_then(|n| n.try_as_number())
                        .and_then(|n| n.to_f64())
                };
                let (x, y) = (side(cidre::cfstr!(c"X")), side(cidre::cfstr!(c"Y")));
                let (rw, rh) = (
                    side(cidre::cfstr!(c"Width")),
                    side(cidre::cfstr!(c"Height")),
                );
                match (x, y, rw, rh) {
                    (Some(x), Some(y), Some(rw), Some(rh)) => (x, y, rw, rh),
                    _ => return false,
                }
            }
            None => {
                // SAFETY: an element of a CF array is an Objective-C object; it is only asked whether it is an NSValue.
                let object = unsafe { &*std::ptr::from_ref(item).cast::<AnyObject>() };
                let Some(value) = object.downcast_ref::<NSValue>() else {
                    return false;
                };
                // SAFETY: `objCType` is a NUL-terminated C string the value keeps for its life.
                let kind = unsafe { std::ffi::CStr::from_ptr(value.objCType().as_ptr()) };
                if !kind.to_bytes().starts_with(b"{CGRect") {
                    return false;
                }
                // SAFETY: the value holds a CGRect (its type encoding says so), which `rectValue` reads.
                let r = unsafe { value.rectValue() };
                (r.origin.x, r.origin.y, r.size.width, r.size.height)
            }
        };
        out.push(CgRect {
            x: rect.0 / w,
            y: rect.1 / h,
            w: rect.2 / w,
            h: rect.3 / h,
        });
    }
    true
}
