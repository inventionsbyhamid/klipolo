//! A measurement's state: one combination measured (`Point`), and the run that measures them
//! (`Run`).

use std::collections::HashMap;
use std::time::Instant;

use super::baseline::Base;
use super::covered::Cut;
use crate::headless::Headless;
use crate::make::budget::{Budget, CostClass};
use crate::make::draft::DraftLayer;
use crate::make::envelope::Params;
use crate::make::measure::{Batches, Decided, Meter, Pcts};

/// One combination measured.
#[derive(Clone, Debug)]
pub(super) struct Point {
    pub(super) params: Params,
    pub(super) preview: Pcts,
    /// Its single frames' p95 less the baseline's: what an export size's frame is predicted from before it is drawn
    /// (`preview` where trusted).
    pub(super) single_p95: f64,
    pub(super) within: bool,
    /// Its own class, and the figure that decided it.
    pub(super) class: CostClass,
    pub(super) decided: Decided,
    pub(super) peak: u64,
    /// Fenced: its batches at preview size and at half size.
    pub(super) batches: Option<Batches>,
    pub(super) half_batches: Option<Batches>,
    /// Fenced: it read Light, but its batches didn't resolve that, so its class is Normal (never Light).
    pub(super) held: bool,
}

pub(super) struct Run<'a> {
    pub(super) meter: &'a mut dyn Meter,
    pub(super) h: &'a mut Headless,
    pub(super) draft: &'a DraftLayer,
    pub(super) frames: usize,
    pub(super) corners: usize,
    pub(super) budget: Budget,
    pub(super) baselines: HashMap<(u32, u32), Base>,
    pub(super) worst: Pcts,
    /// Past it, nothing more is measured: what is left counts as Over.
    pub(super) deadline: Instant,
    /// What the budget or a cap left unmeasured, by cause.
    pub(super) cut: Cut,
    /// The point being measured ran out of time (`point` counts it).
    pub(super) timed_out: bool,
    /// Frames are timed by fences, so in batches (`batched`).
    pub(super) fenced: bool,
}
