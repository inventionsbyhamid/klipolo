//! Parts of the captured picture, in its 0–1 coordinates from the top left.

use serde::{Deserialize, Serialize};

use crate::Us;

/// A rectangle of the captured picture: 0–1 from its top left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect01 {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A part of the picture over `[from, to)`. Its clock is its producer's: the activity's `typing` and `media` give a
/// screen file's own clock, and the take's facts the recording clock (each file's run offset added).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionSpan {
    pub from: Us,
    pub to: Us,
    pub rect: Rect01,
}
