//! The HUD on macOS: a small non-activating panel at the status-bar level, on every Space and over fullscreen games,
//! hidden by default. It never takes a click or the focus, and never shares itself with screen captures or screenshots
//! (`NSWindowSharingNone`; display capture leaves Klipolo's windows out anyway). What it shows and when is the shell
//! view's `HudView`; this draws its frames: a dark pill with the line and the mic's meter, in the profile's corner of
//! the display the take records (a game on a second display has its HUD there), else of the screen the person is on.
//! The HUD changes at most twice a second.

use std::time::Instant;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSBox, NSBoxType, NSColor, NSFont, NSFontWeightSemibold, NSLevelIndicator,
    NSLevelIndicatorStyle, NSPanel, NSScreen, NSStatusWindowLevel, NSTextField, NSTitlePosition,
    NSView, NSWindowCollectionBehavior, NSWindowSharingType, NSWindowStyleMask,
};
use objc2_foundation::{NSNumber, NSPoint, NSRect, NSSize, NSString, ns_string};

use super::own_windows::{self, Own};
use crate::profile::Corner;
use crate::shell::{Flash, StatusView};
use crate::shell_view::{HudFrame, HudView, Words};

/// Space around the line, and between it and the meter, in points.
const PAD: f64 = 12.0;
/// The pill's distance from the screen's edges.
const MARGIN: f64 = 16.0;
const METER: NSSize = NSSize::new(44.0, 8.0);
const TEXT_PT: f64 = 15.0;
const BIG_PT: f64 = 40.0;

pub(super) struct Hud {
    mtm: MainThreadMarker,
    panel: Retained<NSPanel>,
    pill: Retained<NSBox>,
    label: Retained<NSTextField>,
    meter: Retained<NSLevelIndicator>,
    corner: Corner,
    /// The captured display's id (`CGDirectDisplayID`), when the take records one.
    display: Option<u32>,
    view: HudView,
}

impl Hud {
    pub(super) fn new(mtm: MainThreadMarker, corner: Corner, display: Option<String>) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(120.0, 34.0));
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: the panel is owned by this `Retained` and never closed, so AppKit must not release it on close.
        unsafe { panel.setReleasedWhenClosed(false) };
        // The status bar's level, over fullscreen games. Never `setFloatingPanel`, which puts it back to the floating
        // level (the smoke run saw level 3).
        panel.setLevel(NSStatusWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setIgnoresMouseEvents(true);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setHidesOnDeactivate(false);
        panel.setSharingType(NSWindowSharingType::None);
        let content = NSView::initWithFrame(mtm.alloc(), frame);
        let pill = NSBox::initWithFrame(mtm.alloc(), frame);
        pill.setBoxType(NSBoxType::Custom);
        pill.setTitlePosition(NSTitlePosition::NoTitle);
        pill.setBorderWidth(0.0);
        pill.setFillColor(&NSColor::colorWithWhite_alpha(0.08, 0.82));
        let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        label.setTextColor(Some(&NSColor::whiteColor()));
        let meter = NSLevelIndicator::initWithFrame(
            mtm.alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), METER),
        );
        meter.setLevelIndicatorStyle(NSLevelIndicatorStyle::ContinuousCapacity);
        meter.setMinValue(0.0);
        meter.setMaxValue(1.0);
        // The meter's scale is −60 to 0 dBFS: amber from about −9 dBFS, red from about −2.
        meter.setWarningValue(0.85);
        meter.setCriticalValue(0.97);
        content.addSubview(&pill);
        content.addSubview(&label);
        content.addSubview(&meter);
        panel.setContentView(Some(&content));
        Self {
            mtm,
            panel,
            pill,
            label,
            meter,
            corner,
            display: display.and_then(|id| id.parse().ok()),
            view: HudView::default(),
        }
    }

    pub(super) fn status(&mut self, view: StatusView) {
        self.view.status(view, Instant::now());
    }

    pub(super) fn flash(&mut self, kind: Flash) {
        self.view.flash(kind, Instant::now());
    }

    /// How to answer the countdown's question while it holds a take the start key began.
    pub(super) fn answer(&mut self, line: String) {
        self.view.answer(line);
    }

    /// The person pressed their start key (the countdown shows if it starts the take).
    pub(super) fn start_key(&mut self) {
        self.view.start_key();
    }

    /// Draws what changed, if the rate allows; returns when to call again (a flash's end, a held-back change).
    pub(super) fn redraw(&mut self, words: &Words) -> Option<Instant> {
        if let Some(frame) = self.view.frame(words, Instant::now()) {
            self.draw(frame);
        }
        self.view.next_due()
    }

    pub(super) fn hide(&self) {
        self.panel.orderOut(None);
        own_windows::set(Own::Hud, None);
    }

    fn draw(&self, frame: HudFrame) {
        let HudFrame::Shown { text, meter, big } = frame else {
            return self.hide();
        };
        let size = if big { BIG_PT } else { TEXT_PT };
        // SAFETY: reading AppKit's font-weight constant.
        let weight = unsafe { NSFontWeightSemibold };
        self.label
            .setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                size, weight,
            )));
        self.label.setStringValue(&NSString::from_str(&text));
        self.label.sizeToFit();
        let line = self.label.frame().size;
        let meter_w = meter.map_or(0.0, |_| PAD + METER.width);
        let size = NSSize::new(PAD + line.width + meter_w + PAD, line.height + PAD);
        self.label
            .setFrameOrigin(NSPoint::new(PAD, (size.height - line.height) / 2.0));
        self.meter.setHidden(meter.is_none());
        if let Some(level) = meter {
            self.meter.setDoubleValue(f64::from(level));
            let at = NSPoint::new(PAD + line.width + PAD, (size.height - METER.height) / 2.0);
            self.meter.setFrameOrigin(at);
        }
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), size);
        self.pill.setFrame(bounds);
        self.pill.setCornerRadius(size.height.min(24.0) / 2.0);
        let placed = self.placed(size);
        self.panel.setFrame_display(placed, true);
        // Before it shows: the screen's frames that changed only the HUD are idle ones (`own_windows`).
        own_windows::set(Own::Hud, Some(placed));
        self.panel.orderFrontRegardless();
    }

    /// The pill's frame in the profile's corner of the captured display, else of the screen the person is on (the key
    /// screen, else the main one).
    fn placed(&self, size: NSSize) -> NSRect {
        let screen = (self
            .captured_screen()
            .or_else(|| NSScreen::mainScreen(self.mtm)))
        .map(|s| s.visibleFrame())
        .unwrap_or(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(1440.0, 900.0),
        ));
        let (left, bottom) = (screen.origin.x + MARGIN, screen.origin.y + MARGIN);
        let right = screen.origin.x + screen.size.width - MARGIN - size.width;
        let top = screen.origin.y + screen.size.height - MARGIN - size.height;
        let origin = match self.corner {
            Corner::TopLeft => NSPoint::new(left, top),
            Corner::TopRight => NSPoint::new(right, top),
            Corner::BottomLeft => NSPoint::new(left, bottom),
            Corner::BottomRight => NSPoint::new(right, bottom),
        };
        NSRect::new(origin, size)
    }

    /// The screen showing the captured display, found by its `NSScreenNumber` (None if it was unplugged).
    fn captured_screen(&self) -> Option<Retained<NSScreen>> {
        let wanted = self.display?;
        NSScreen::screens(self.mtm).into_iter().find(|screen| {
            (screen.deviceDescription())
                .objectForKey(ns_string!("NSScreenNumber"))
                .and_then(|n| n.downcast_ref::<NSNumber>().map(NSNumber::unsignedIntValue))
                == Some(wanted)
        })
    }
}
