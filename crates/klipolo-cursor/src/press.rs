//! A button press, down to up.

use serde::{Deserialize, Serialize};

use crate::Us;

/// A button press: down at `down`, up at `up` (`Us::MAX` while still held at the file's end), where it went down.
/// `btn`: 0 primary, 1 secondary, 2 middle, 3 and up the others.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Press {
    pub down: Us,
    pub up: Us,
    pub btn: u8,
    pub x: f32,
    pub y: f32,
}
