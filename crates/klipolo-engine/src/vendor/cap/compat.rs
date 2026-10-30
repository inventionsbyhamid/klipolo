// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/project/src/configuration.rs (excerpts: 80–222 XY and its operator impls, 211–216 Crop, 465–515 ColorCorrection, 794–810 ScreenMovementSpring, 927–980 TimelineSegment, 980–1025 GlideDirection/ZoomSegment/ZoomMode, 1120–1160 TextAlign/TextBackgroundStyle) and crates/project/src/cursor.rs (excerpts: 14–35 CursorMoveEvent/CursorClickEvent, 68–72 CursorEvents)
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: specta `Type` and serde derives dropped (with ZoomSegment's serde default fns); `FrameTarget` added (Klipolo's stand-in for the output size and frame clock the vendored frame pipeline read from Cap's `ProjectUniforms`); TimelineSegment keeps only the fields zoom_spring.rs reads (recording_clip, timescale, start, end) and `duration`; TimelineConfiguration is reduced to `segments` (Cap's has transitions, zoom/scene/style/text… segments).

//! Minimal stand-ins for cap_project types the vendored files use (copied field for field; see each type).

use std::ops::{Add, Div, Mul, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XY<T> {
    pub x: T,
    pub y: T,
}

impl<T> XY<T> {
    pub const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    pub fn map<U, F: Fn(T) -> U>(self, f: F) -> XY<U> {
        XY {
            x: f(self.x),
            y: f(self.y),
        }
    }
}

impl<T: Add<Output = T>> Add for XY<T> {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }
}

impl<T: Sub<Output = T>> Sub for XY<T> {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}

impl<T: Sub<Output = T> + Copy> Sub<T> for XY<T> {
    type Output = Self;

    fn sub(self, other: T) -> Self {
        Self {
            x: self.x - other,
            y: self.y - other,
        }
    }
}

impl<T: Mul<Output = T> + Copy> Mul<XY<T>> for XY<T> {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self {
            x: self.x * other.x,
            y: self.y * other.y,
        }
    }
}

impl<T: Mul<Output = T> + Copy> Mul<T> for XY<T> {
    type Output = Self;

    fn mul(self, other: T) -> Self {
        Self {
            x: self.x * other,
            y: self.y * other,
        }
    }
}

impl<T: Div<Output = T> + Copy> Div<T> for XY<T> {
    type Output = Self;

    fn div(self, other: T) -> Self {
        Self {
            x: self.x / other,
            y: self.y / other,
        }
    }
}

impl<T: Div<Output = T>> Div<XY<T>> for XY<T> {
    type Output = Self;

    fn div(self, other: XY<T>) -> Self {
        Self {
            x: self.x / other.x,
            y: self.y / other.y,
        }
    }
}

impl<T> SubAssign for XY<T>
where
    T: SubAssign + Copy,
{
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl From<XY<f32>> for XY<f64> {
    fn from(val: XY<f32>) -> Self {
        XY {
            x: val.x as f64,
            y: val.y as f64,
        }
    }
}

impl<T> From<(T, T)> for XY<T> {
    fn from(val: (T, T)) -> Self {
        XY { x: val.0, y: val.1 }
    }
}

/// Parametric color grade for a single layer (screen or camera). Every field
/// except `intensity` has 0 as its identity, so a default struct renders
/// exactly like no grade at all. Adjustment fields are normalized: -1..1 for
/// bipolar controls, 0..1 for unipolar ones.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorCorrection {
    /// UI preset id ("none", "cinematic", ..., or "custom"). The renderer
    /// ignores this; the numeric fields below are the source of truth.
    pub preset: String,
    /// 0..1 master strength applied to every adjustment except `grain`,
    /// which has its own dedicated control.
    pub intensity: f32,
    /// -1..1, full scale is ±1.5 stops.
    pub exposure: f32,
    /// -1..1 around a mid-gray pivot.
    pub contrast: f32,
    /// -1..1; -1 is grayscale.
    pub saturation: f32,
    /// -1..1; positive warms, negative cools.
    pub temperature: f32,
    /// -1..1; positive shifts magenta, negative green.
    pub tint: f32,
    /// 0..1 lifted-blacks film fade.
    pub fade: f32,
    /// -1..1 teal-shadows/orange-highlights split toning (negative reverses).
    pub split_tone: f32,
    /// 0..1 edge darkening within the layer's own rect.
    pub vignette: f32,
    /// 0..1 animated film grain.
    pub grain: f32,
}

impl ColorCorrection {
    pub const PRESET_NONE: &'static str = "none";
}

impl Default for ColorCorrection {
    fn default() -> Self {
        Self {
            preset: Self::PRESET_NONE.to_string(),
            intensity: 1.0,
            exposure: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
            fade: 0.0,
            split_tone: 0.0,
            vignette: 0.0,
            grain: 0.0,
        }
    }
}

/// What the vendored frame pipeline needs from a frame: its output size and its place on the frame clock
/// (Cap read these from `ProjectUniforms`).
#[derive(Clone, Copy, Debug)]
pub struct FrameTarget {
    pub output_size: (u32, u32),
    pub frame_number: u32,
    pub frame_rate: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Crop {
    pub position: XY<u32>,
    pub size: XY<u32>,
}

impl Crop {
    pub fn aspect_ratio(&self) -> f32 {
        self.size.x as f32 / self.size.y as f32
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScreenMovementSpring {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

impl Default for ScreenMovementSpring {
    fn default() -> Self {
        Self {
            stiffness: 200.0,
            damping: 40.0,
            mass: 2.25,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TimelineSegment {
    pub recording_clip: u32,
    pub timescale: f64,
    pub start: f64,
    pub end: f64,
}

impl TimelineSegment {
    /// in seconds
    pub fn duration(&self) -> f64 {
        (self.end - self.start) / self.timescale
    }
}

#[derive(Clone, Debug, Default)]
pub struct TimelineConfiguration {
    pub segments: Vec<TimelineSegment>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Copy)]
pub enum GlideDirection {
    #[default]
    None,
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub struct ZoomSegment {
    pub start: f64,
    pub end: f64,
    pub amount: f64,
    pub mode: ZoomMode,
    pub glide_direction: GlideDirection,
    pub glide_speed: f64,
    pub instant_animation: bool,
    pub edge_snap_ratio: f64,
}

#[derive(Clone, Debug)]
pub enum ZoomMode {
    Auto,
    Manual { x: f32, y: f32 },
}

#[derive(Debug, Clone)]
pub struct CursorMoveEvent {
    pub active_modifiers: Vec<String>,
    pub cursor_id: String,
    pub time_ms: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone)]
pub struct CursorClickEvent {
    pub active_modifiers: Vec<String>,
    pub cursor_num: u8,
    pub cursor_id: String,
    pub time_ms: f64,
    pub down: bool,
}

#[derive(Default, Debug, Clone)]
pub struct CursorEvents {
    pub clicks: Vec<CursorClickEvent>,
    pub moves: Vec<CursorMoveEvent>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextBackgroundStyle {
    /// One rounded rectangle behind the whole block.
    #[default]
    Box,
    /// A capsule: half-height radius and wider side padding.
    Pill,
    /// A marker stroke per laid-out line, hugging each line's ink.
    Highlight,
}
