//! The smart camera: Auto zooms planned for a whole span ahead of time,
//! pure, on one time axis. The engine plans its Auto zoom layers with it and chat plans the layers it makes
//! so chat's zooms play as it planned them.
//!
//! `plan` turns the focus into shots (`shots`), places them with legal moves only (`legal`: a glide, a wider view, an
//! out-and-in, a merge, else the lower-priority shot is dropped; never a move faster than its limits), and keys the
//! path (`moves`). `with_glide` then lets each shot follow the raw cursor inside its safe zone (`glide`). The plan is
//! sampled by binary search with no allocation, deterministically (stable sorts, f64 in a fixed order).

mod ease;
mod focus;
mod glide;
mod legal;
mod moves;
mod rules;
mod safe;
mod seen;
mod shots;

#[cfg(feature = "test-util")]
mod check;

#[cfg(feature = "test-util")]
pub use check::{check_plan, plan_capped};
pub use focus::{focus_from, keep_from};
pub use moves::Key;
pub use rules::Rules;
#[cfg(feature = "test-util")]
pub use seen::MAX_ROUNDS;

use crate::{Rect01, Us};

/// A view of the source frame: its centre (0–1 from the top left) and its zoom (≥ 1; 1 shows the whole frame).
/// `View::FULL` and `View::rect` live with the safe zone (`safe`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub cx: f64,
    pub cy: f64,
    pub scale: f64,
}

/// How much a focus matters when two can't both be framed: the lower is dropped (ties: the later).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Weight {
    Dwell,
    Typing,
    Click,
    Said,
    Person,
}

/// Something to frame: a click (`at == until`, a point), a pause, typing, words. On the plan's time axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Focus {
    pub at: Us,
    pub until: Us,
    pub rect: Rect01,
    pub weight: Weight,
}

/// The person's own zoom over `[from, to)`: never moved, never dropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fixed {
    pub from: Us,
    pub to: Us,
    pub view: View,
}

/// What to plan: the focus and the person's zooms over `span` (`[start, end)`; the whole frame at both ends).
pub struct Input<'a> {
    pub span: (Us, Us),
    pub focus: &'a [Focus],
    pub fixed: &'a [Fixed],
    pub rules: Rules,
}

/// A kept shot: its view from its first event to its hold's end. `events` counts the focus it frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub from: Us,
    pub to: Us,
    pub view: View,
    pub weight: Weight,
    pub events: u16,
}

/// The conflicts the ladder resolved: kept shots framed wider than they would be alone, links made legal by an
/// out-and-in (rule 5's zoom-outs aren't conflicts), shots merged, and the focus no kept shot frames (a dropped shot
/// counts each of its events).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    pub widened: u32,
    pub out_and_in: u32,
    pub merged: u32,
    pub dropped: u32,
}

/// The camera's path over a span: its keys, kept shots and their events, and each shot's glide.
#[derive(Debug, Default)]
pub struct Plan {
    rules: Rules,
    keys: Vec<Key>,
    shots: Vec<Shot>,
    holds: Vec<moves::Hold>,
    events: Vec<(Us, f32, f32)>,
    /// Each event's `(at, until, rect)`, as its focus gave them.
    boxes: Vec<(Us, Us, Rect01)>,
    /// The dropped actions the view shows (outside the person's zooms), in time order.
    in_view: Vec<seen::Point>,
    /// The focus in the span as given (and what it keeps in view), for `check_plan` to find the dropped actions itself.
    #[cfg(feature = "test-util")]
    all: Vec<Focus>,
    /// The span it was planned over, for `check_plan`'s squeezed moves.
    #[cfg(feature = "test-util")]
    span: (Us, Us),
    glide: glide::Glide,
    ladder: Dropped,
}

/// O(n log n) in focus + fixed; deterministic (stable sorts, f64 in a fixed order).
pub fn plan(input: &Input) -> Plan {
    plan_keeping(input, &[])
}

/// `plan`, also keeping `keep` on screen: actions that never get a shot of their own (the kinds a narrowed `using`
/// leaves out, `keep_from`), held in view as a dropped action is, so neither a shot nor its glide hides one. They
/// aren't focus, so `Dropped` doesn't count them. O(n log n) in focus + keep + fixed.
pub fn plan_keeping(input: &Input, keep: &[Focus]) -> Plan {
    build(input, keep, seen::MAX_ROUNDS).0
}

/// `plan_keeping` with its repair capped at `cap` rounds, and how many rounds it ran.
fn build(input: &Input, keep: &[Focus], cap: u32) -> (Plan, u32) {
    let (r, (start, end)) = (input.rules.effective(), input.span);
    let mut p = Plan {
        rules: r,
        ..Plan::default()
    };
    let in_span = |f: &&Focus| f.at >= start && f.at < end;
    #[cfg(feature = "test-util")]
    {
        p.all.extend(input.focus.iter().chain(keep).filter(in_span));
        p.span = input.span;
    }
    if !r.sane() || end <= start {
        return (p, 0);
    }
    let (focus, mut dropped, shots) = shots::prepare(input, keep, &r);
    let (kept, merged) = legal::ladder(shots, &r, &mut dropped, &focus);
    let (kept, in_view, mut ladder, rounds) =
        seen::repair(kept, merged, &focus, &mut dropped, &r, cap);
    ladder.dropped -= keep.iter().filter(in_span).count() as u32;
    (p.in_view, p.ladder) = (in_view, ladder);
    moves::fill(&mut p, &kept, &focus, &dropped);
    (p, rounds)
}
