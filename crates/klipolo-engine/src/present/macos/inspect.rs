//! How the native preview sits in the window, read back from AppKit: the dev-only trial checks it in the packaged
//! app, where nothing may capture the screen, and `tests/native_view.rs` checks it in an off-screen window.

use objc2::MainThreadMarker;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::INSTALLED;

/// What the native preview looks like in the window. Main thread only; `None` when nothing is installed.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    /// The preview view is a sibling of the webview, ordered below it.
    pub below_webview: bool,
    pub hidden: bool,
    /// The view's frame in the page's coordinates (CSS px), to compare with the hole the UI sent.
    pub frame: [f64; 4],
    /// Where the layer itself is, in the page's coordinates: what actually shows through the hole. AppKit places a
    /// hosted layer with its view; a layer moved on its own shows somewhere else whatever the view's frame says.
    pub layer_frame: [f64; 4],
    /// The layer's size in points (its bounds).
    pub layer_size: [f64; 2],
    /// The hole the UI sent (CSS px, top-left origin) and the devicePixelRatio it measured it at.
    pub hole: Option<[f64; 4]>,
    pub hole_scale: Option<f64>,
    /// The window's backing scale factor (0: not in a window).
    pub backing_scale: f64,
    pub contents_scale: f64,
    /// How the drawable maps onto the layer's bounds (`resize`: scaled to fill them).
    pub contents_gravity: String,
    /// The drawable's size in pixels: the compositor's output size (the presenter sets it from each frame's slot).
    pub drawable_size: [f64; 2],
    /// WKWebView is flipped (top-left origin, like the CSS rect); the conversion relies on AppKit knowing it.
    pub webview_flipped: bool,
    /// Where the page's CSS (0, 0) is in the webview (WebKit's content insets): (0, 28) under the title bar of
    /// Tauri's default window, (0, 0) in a window whose content stops at its title bar.
    pub page_origin: [f64; 2],
    /// False: the webview paints only what the page paints, so the hole is see-through (None: can't tell).
    pub webview_draws_background: Option<bool>,
    pub window_opaque: bool,
    /// The view never takes a click.
    pub hit_test_nil: bool,
    /// What is wrong with the picture (empty: the preview fills the hole), from `problems`.
    pub problems: Vec<String>,
}

pub fn inspect() -> Option<Inspection> {
    MainThreadMarker::new()?;
    INSTALLED.with_borrow(|installed| {
        let i = installed.as_ref()?;
        let parent = unsafe { i.view.superview() }?;
        let subviews = parent.subviews();
        // SAFETY: NSArray's `count` and `objectAtIndex:` within bounds.
        let index = |v: &NSView| -> Option<usize> {
            let count: usize = unsafe { msg_send![&*subviews, count] };
            (0..count).find(|&k| {
                let at: *mut NSView = unsafe { msg_send![&*subviews, objectAtIndex: k] };
                std::ptr::eq(at, v)
            })
        };
        let below_webview = match (index(&i.view), index(&i.webview)) {
            (Some(view), Some(webview)) => view < webview,
            _ => false,
        };
        let origin = super::page_origin(&i.webview);
        let origin = [origin.x, origin.y];
        // From the parent's coordinates to the page's, the way `set_rect` went the other way.
        let page = |r: NSRect| {
            let r = i.webview.convertRect_fromView(r, Some(&parent));
            page_rect(
                [r.origin.x, r.origin.y, r.size.width, r.size.height],
                origin,
            )
        };
        let frame = page(i.view.frame());
        let draws = super::draws_background(&i.webview);
        // SAFETY: CALayer/CAMetalLayer properties; `hitTest:` with a point inside.
        let (layer_frame, layer_bounds, gravity, drawable, hit) = unsafe {
            // In the superlayer's coordinates, which for a view's layer are its superview's.
            let layer_frame: NSRect = msg_send![i.layer.object(), frame];
            let layer_bounds: NSRect = msg_send![i.layer.object(), bounds];
            let gravity: Retained<NSString> = msg_send![i.layer.object(), contentsGravity];
            let drawable: NSSize = msg_send![i.layer.object(), drawableSize];
            let centre = NSPoint::new(
                i.view.frame().origin.x + i.view.frame().size.width / 2.0,
                i.view.frame().origin.y + i.view.frame().size.height / 2.0,
            );
            let hit: *mut NSView = msg_send![&*i.view, hitTest: centre];
            (
                layer_frame,
                layer_bounds,
                gravity.to_string(),
                drawable,
                hit,
            )
        };
        let window = i.view.window();
        let mut inspection = Inspection {
            below_webview,
            hidden: i.view.isHidden(),
            frame,
            layer_frame: page(layer_frame),
            layer_size: [layer_bounds.size.width, layer_bounds.size.height],
            hole: i.rect.map(|r| [r.x, r.y, r.width, r.height]),
            hole_scale: i.rect.map(|r| r.scale),
            backing_scale: window.as_ref().map_or(0.0, |w| w.backingScaleFactor()),
            contents_scale: i.layer.layer().contents_scale(),
            contents_gravity: gravity,
            drawable_size: [drawable.width, drawable.height],
            webview_flipped: i.webview.isFlipped(),
            page_origin: origin,
            webview_draws_background: draws,
            window_opaque: window.is_some_and(|w| w.isOpaque()),
            hit_test_nil: hit.is_null(),
            problems: Vec::new(),
        };
        inspection.problems = problems(&inspection);
        Some(inspection)
    })
}

/// A rect in the webview's own coordinates as the page sees it: the page starts at `origin` in the webview.
fn page_rect([x, y, w, h]: [f64; 4], [ox, oy]: [f64; 2]) -> [f64; 4] {
    [x - ox, y - oy, w, h]
}

/// Within half a point: the hole is snapped to device pixels, so a correct placement matches it exactly.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.5
}

/// What would make the preview wrong on screen (empty when it fills the hole): the layer (not only its view) over the
/// hole at the hole's size, Retina-sharp, and the whole frame scaled into it.
fn problems(i: &Inspection) -> Vec<String> {
    let mut out = Vec::new();
    let Some(hole) = i.hole else {
        if !i.hidden {
            out.push("no hole, yet the preview shows".into());
        }
        return out;
    };
    let same = |r: &[f64; 4]| r.iter().zip(hole).all(|(a, b)| near(*a, b));
    if i.hidden {
        out.push("the preview is hidden".into());
    }
    if !same(&i.frame) {
        out.push(format!(
            "the view is at {:?}, not over the hole {hole:?}",
            i.frame
        ));
    }
    if !same(&i.layer_frame) {
        out.push(format!(
            "the layer is at {:?}, not over the hole {hole:?}",
            i.layer_frame
        ));
    }
    if !near(i.layer_size[0], hole[2]) || !near(i.layer_size[1], hole[3]) {
        out.push(format!(
            "the layer is {:?} points, the hole {:?}",
            i.layer_size,
            [hole[2], hole[3]]
        ));
    }
    if i.backing_scale > 0.0 && i.contents_scale != i.backing_scale {
        out.push(format!(
            "contentsScale {} on a {}x window",
            i.contents_scale, i.backing_scale
        ));
    }
    if i.contents_gravity != "resize" {
        out.push(format!(
            "contentsGravity {}: the frame doesn't fill the layer",
            i.contents_gravity
        ));
    }
    // A drawable of 1×1 or less is a released one (hidden, nothing open). Otherwise the compositor's frame, of the
    // hole's shape (its size is capped by the pixel budget, and Core Animation scales it to the layer).
    let [w, h] = i.drawable_size;
    if w > 1.0 && h > 1.0 && hole[2] > 0.0 && hole[3] > 0.0 {
        let (frame, layer) = (w / h, hole[2] / hole[3]);
        // Both sides are rounded (to even pixels, to device pixels): allow a pixel or two of either.
        let slack = 2.0 / h + 2.0 / (hole[3] * i.contents_scale.max(1.0));
        if (frame - layer).abs() > layer * slack {
            out.push(format!(
                "the {w}×{h} frame is not the hole's shape {:?}",
                [hole[2], hole[3]]
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view over the hole, its layer at the content view's corner.
    fn layer_at_corner() -> Inspection {
        Inspection {
            below_webview: true,
            hidden: false,
            frame: [370.0, 149.5, 717.0, 403.0],
            layer_frame: [0.0, 444.0, 717.0, 403.0],
            layer_size: [717.0, 403.0],
            hole: Some([370.0, 149.5, 717.0, 403.0]),
            hole_scale: Some(2.0),
            backing_scale: 2.0,
            contents_scale: 2.0,
            contents_gravity: "resize".into(),
            drawable_size: [1280.0, 718.0],
            webview_flipped: true,
            page_origin: [0.0, 0.0],
            webview_draws_background: Some(false),
            window_opaque: true,
            hit_test_nil: true,
            problems: Vec::new(),
        }
    }

    #[test]
    fn a_layer_away_from_its_hole_is_a_problem_even_when_the_view_is_over_it() {
        let found = problems(&layer_at_corner());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("the layer is at"), "{found:?}");
        let placed = Inspection {
            layer_frame: [370.0, 149.5, 717.0, 403.0],
            ..layer_at_corner()
        };
        assert!(problems(&placed).is_empty(), "{:?}", problems(&placed));
    }

    /// Under Tauri's default title bar WebKit starts the page 28 pt down the webview. A view placed at the
    /// hole's CSS numbers in the webview's own coordinates (what `set_rect` did) is 28 pt above the hole.
    #[test]
    fn a_view_at_the_holes_numbers_under_a_title_bar_inset_is_above_the_hole() {
        let origin = [0.0, 28.0];
        let hole = [370.0, 149.5, 717.0, 403.0];
        let at_numbers = page_rect(hole, origin);
        assert_eq!(at_numbers, [370.0, 121.5, 717.0, 403.0]);
        let above = Inspection {
            frame: at_numbers,
            layer_frame: at_numbers,
            page_origin: origin,
            ..layer_at_corner()
        };
        let found = problems(&above);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found[0].starts_with("the view is at [370.0, 121.5"),
            "{found:?}"
        );
        // 28 pt lower in the webview, it is over the hole.
        let over = page_rect([370.0, 177.5, 717.0, 403.0], origin);
        assert_eq!(over, hole);
        let placed = Inspection {
            frame: over,
            layer_frame: over,
            page_origin: origin,
            ..layer_at_corner()
        };
        assert!(problems(&placed).is_empty(), "{:?}", problems(&placed));
    }

    #[test]
    fn scale_gravity_and_the_frames_shape_are_checked() {
        let placed = Inspection {
            layer_frame: [370.0, 149.5, 717.0, 403.0],
            ..layer_at_corner()
        };
        let blurry = Inspection {
            contents_scale: 1.0,
            ..placed.clone()
        };
        assert_eq!(problems(&blurry).len(), 1);
        let unscaled = Inspection {
            contents_gravity: "center".into(),
            ..placed.clone()
        };
        assert_eq!(problems(&unscaled).len(), 1);
        let squashed = Inspection {
            drawable_size: [1280.0, 960.0],
            ..placed.clone()
        };
        assert_eq!(problems(&squashed).len(), 1);
        let released = Inspection {
            drawable_size: [1.0, 1.0],
            ..placed
        };
        assert!(problems(&released).is_empty());
    }
}
