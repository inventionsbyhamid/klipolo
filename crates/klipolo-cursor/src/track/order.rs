//! The per-kind order check. Events from different threads arrive out of order: a button
//! stamped by the hardware can be written after a later heartbeat. So each kind is checked only against itself: moves
//! against moves, each button against itself, shapes, visibility, areas, click modes and burn-ins each on their own.
//! Equal times are kept; only a line older than its own kind's last is skipped (and counted).

use super::LastTimes;
use crate::Us;
use crate::format::Line;

/// The kinds lines are ordered within, buttons one each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Move,
    Button(u8),
    Shape,
    Visible,
    Area,
    Clicks,
    Burned,
}

impl Kind {
    /// A timed line's kind and time; None for the header and shape definitions.
    pub(super) fn of(line: &Line) -> Option<(Self, Us)> {
        Some(match *line {
            Line::Move { t, .. } => (Self::Move, t),
            Line::Button { t, btn, .. } => (Self::Button(btn), t),
            Line::Shape { t, .. } => (Self::Shape, t),
            Line::Visible { t, .. } => (Self::Visible, t),
            Line::Area { t, .. } => (Self::Area, t),
            Line::Clicks { t, .. } => (Self::Clicks, t),
            Line::Burned { t } => (Self::Burned, t),
            Line::Header { .. } | Line::ShapeDef { .. } => return None,
        })
    }

    /// A kind the track keeps as a 16-byte span or change: shapes, visibility, areas and click modes.
    pub(super) fn is_span(self) -> bool {
        matches!(
            self,
            Self::Shape | Self::Visible | Self::Area | Self::Clicks
        )
    }
}

/// Each kind's last time.
pub(super) struct Order {
    last: [Us; 6],
    buttons: [Us; 256],
}

impl Default for Order {
    fn default() -> Self {
        Self {
            last: [Us::MIN; 6],
            buttons: [Us::MIN; 256],
        }
    }
}

impl Order {
    /// Whether a line of `kind` at `t` is in order (no older than its kind's last); if so it becomes the last.
    pub(super) fn admit(&mut self, kind: Kind, t: Us) -> bool {
        let last = match kind {
            Kind::Button(b) => &mut self.buttons[usize::from(b)],
            Kind::Move => &mut self.last[0],
            Kind::Shape => &mut self.last[1],
            Kind::Visible => &mut self.last[2],
            Kind::Area => &mut self.last[3],
            Kind::Clicks => &mut self.last[4],
            Kind::Burned => &mut self.last[5],
        };
        let ok = t >= *last;
        if ok {
            *last = t;
        }
        ok
    }

    /// Each kind's last time (`Us::MIN`, the start, means none came).
    pub(super) fn last_times(&self) -> LastTimes {
        let some = |t: Us| (t != Us::MIN).then_some(t);
        LastTimes {
            moves: some(self.last[0]),
            buttons: self.buttons.iter().copied().max().and_then(some),
            shapes: some(self.last[1]),
            visible: some(self.last[2]),
            areas: some(self.last[3]),
            clicks: some(self.last[4]),
            burned: some(self.last[5]),
        }
    }
}
