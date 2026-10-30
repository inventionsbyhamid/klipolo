//! Where a generator layer's piece sits in its contribution's time. A scene's segments map onto the whole
//! layer it was placed as: a split gives both pieces that span, the second one starting `offset_us` into it, so the
//! pieces continue each other; an unsplit layer has no span and maps onto its own length (trims keep its entrance at
//! its start and its exit at its end). The rules for splits, trims and cuts are `apply::generator_pieces`'s. A split's
//! pieces also keep the whole layer's id to seed their effects from (`seed_id`).

use std::collections::BTreeMap;

use super::{Item, ItemContent};
use crate::contrib::ContribId;
use crate::params::ParamValue;
use crate::time::Us;

/// `offset_us` is left out of the JSON while 0.
pub(super) fn is_zero_us(us: &Us) -> bool {
    *us == 0
}

impl ItemContent {
    /// A generator (or scene) layer with these settings, drawn moving, not split.
    pub fn generator(source: ContribId, params: BTreeMap<String, ParamValue>) -> Self {
        Self::Generator {
            source,
            params,
            still: false,
            offset_us: 0,
            span_us: None,
            seed_from: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PieceTime {
    pub offset_us: Us,
    pub span_us: Us,
}

/// The id a generator layer's effects are seeded from (`kl.seed`): the layer it was split from, else its own; any other
/// layer's own id.
pub fn seed_id(item: &Item) -> &str {
    match &item.content {
        ItemContent::Generator {
            seed_from: Some(id),
            ..
        } => id,
        _ => &item.id,
    }
}

/// Where a generator layer's piece sits in its contribution's time: its offset, and the whole span the
/// contribution's segments map onto (its own length when it was never split).
pub fn piece_time(item: &Item) -> Option<PieceTime> {
    match &item.content {
        ItemContent::Generator {
            offset_us, span_us, ..
        } => Some(PieceTime {
            offset_us: *offset_us,
            span_us: span_us.unwrap_or(item.duration_us),
        }),
        _ => None,
    }
}
