//! The speaker's path for one format's crop, from a tracker's boxes (`TrackedFrame`s, each with its time in the clip's
//! source time: from one tracked stretch or several):
//!
//! - **The area** is what the clip's crop keeps of the picture (all of it when uncropped). The caller leaves out anyone
//!   whose middle is outside it; the subject is picked near its middle, and the crop stays inside it.
//! - **Shots** run between the cuts. Each picks its own subject (`main_subject`): the person with the most area near
//!   the area's middle over the shot, so the largest, most central one. Someone else who is there for most of the shot and at
//!   least half as big joins them while both fit in the crop (a two-person shot is framed together).
//! - **The aim** on a frame is the subject's middle across and, down, a crop whose top sits just above their head
//!   (`HEADROOM`): their head and upper body. Between two frames it moves straight from one to the next; while the
//!   subject is gone it holds where they were last, and before they first show, where they first do.
//! - **The crop** starts each shot at rest where the subject is in its first two seconds, and holds while the aim stays
//!   inside its central dead zone. Once the aim leaves it, the crop moves (critically damped at `OMEGA`, through the
//!   cursor follow's own filter) until it is back on the aim, never faster than `SPEED` crop widths a second and never
//!   turning back mid-move, so a tracker's wobble can't make it jitter.
//! - **Keys** every `KEY_STEP_US` from each shot's start while the crop moves, and only the first and last of a hold
//!   (the crop's centre goes straight between them, so it draws the same), so a long still talk stays a few keys a
//!   shot. A shot's first key is a cut, so the crop jumps there and never pans from one shot to the next. A shot with
//!   nobody in it holds the area's middle.
//! - **Two or more people** (`people`: the main subject and those framed with them, left to right) who don't all fit
//!   the crop: it keeps whoever is speaking (`Talk`). A new speaker takes the frame only after `MIN_TALK_US` of talk
//!   (a "mm-hm" never does), at least `MIN_HOLD_US` after the last switch. Short turns back and forth, or talk over
//!   each other, frame everyone in it when they fit, else whoever said more there. Silence, or a voice nobody on
//!   screen has, holds who is framed. A shot opens on its first speaker (else the last one, else its main subject). A
//!   switch further than `CUT_AWAY` crop widths is a cut at the start of the speech; a nearer one eases. `Keep` can
//!   name one person, everyone, or who when instead (a moment at a shot's edge those ranges leave out or cover, too
//!   short for anyone's talk to count, goes with its neighbour). Without anyone heard, or with everyone fitting, a shot is framed
//!   as above.

use klipolo_cursor::smooth::step;
use klipolo_model::fill_focus::{FocusKey, FocusPath, Keep, MAX_PEOPLE, Who};
use klipolo_model::layout::BoxN;
use klipolo_model::time::Us;

use crate::providers::{TrackedBox, TrackedFrame};

/// The crop holds while the aim is within this part of its size around its centre (half of it each side).
pub const DEAD_ZONE: f64 = 0.4;
/// A move ends once the crop is this close to the aim, in crop sizes.
pub const SETTLE: f64 = 0.04;
/// The fastest the crop moves, in crop sizes a second.
pub const SPEED: f64 = 0.6;
/// The follower's ω, 1/s (the cursor follow's).
pub const OMEGA: f64 = 3.0;
/// Room above the head, as a part of the crop's height.
pub const HEADROOM: f64 = 0.1;
/// A key every quarter second.
pub const KEY_STEP_US: Us = 250_000;
/// Boxes the tracker is less sure of than this are left out.
pub const MIN_CONFIDENCE: f32 = 0.4;
/// The crop starts each shot where the subject is over this long.
const START_US: Us = 2_000_000;
/// The motion's step.
const DT_US: Us = 1_000_000 / 30;
/// Two frames further apart than this aren't moved between: the aim holds.
const MAX_GAP_US: Us = 1_000_000;
/// A new speaker takes the frame only after talking this long.
pub const MIN_TALK_US: Us = 1_500_000;
/// The crop stays on someone at least this long before it switches to another speaker.
pub const MIN_HOLD_US: Us = 2_000_000;
/// A switch to someone further away than this many crop widths is a cut; a nearer one eases.
pub const CUT_AWAY: f64 = 0.5;
/// One person's talk with gaps shorter than this is one run.
const RUN_GAP_US: Us = 300_000;
/// Short turns and talk over each other this close together are one busy stretch.
const BUSY_GAP_US: Us = 1_000_000;
/// Someone takes part in a busy stretch with at least this much talk in it.
const MIN_SHARE_US: Us = 500_000;
/// A switch that would leave less of the speech than this isn't made.
const MIN_LEFT_US: Us = 1_000_000;
/// People fit together in the crop when they take at most this part of its width.
const FIT: f64 = 0.9;

/// Someone on screen speaking, in the frames' time: `person` counts from 1 at the left of the shot's `people`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Talk {
    pub start_us: Us,
    pub end_us: Us,
    pub person: u8,
}

/// What a path is made from.
#[derive(Clone, Copy, Debug)]
pub struct PathInput<'a> {
    /// The tracker's frames, each at its time in the clip's source time, in time order.
    pub frames: &'a [(Us, &'a TrackedFrame)],
    /// Where the footage the frames cover starts, in the same time.
    pub start_us: Us,
    /// Where the footage the frames cover ends, in the same time.
    pub end_us: Us,
    /// The footage's cuts (each the first frame of a new shot), sorted, in the same time.
    pub cuts: &'a [Us],
    /// The crop's size, as parts of the picture's width and height.
    pub crop: (f64, f64),
    /// What the clip's crop keeps of the picture, where the file has it (`FULL` when uncropped): the subject is picked
    /// near its middle and the crop stays inside it.
    pub area: BoxN,
    /// Who on screen speaks when, in the same time; empty when nobody was heard.
    pub talk: &'a [Talk],
    /// Who to keep in frame where two or more people are in a shot.
    pub keep: &'a Keep,
}

/// The whole picture.
pub const FULL: BoxN = BoxN {
    x: 0.0,
    y: 0.0,
    w: 1.0,
    h: 1.0,
};

/// The path, or None when nobody was found on any frame.
pub fn focus_path(input: &PathInput) -> Option<FocusPath> {
    if input.end_us <= input.start_us {
        return None;
    }
    let area = sane_area(input.area);
    let crop = (
        sane_side(input.crop.0).min(area.w),
        sane_side(input.crop.1).min(area.h),
    );
    let timed = input.frames;
    let shots = shots(input.start_us, input.end_us, input.cuts);
    // Fewer keys a second on very long footage, to stay within the path's cap.
    let room = FocusPath::MAX_KEYS.saturating_sub(shots.len()).max(1) as Us;
    let key_step = KEY_STEP_US.max((input.end_us - input.start_us) / room + 1);

    let mut keys = Vec::new();
    let mut found = false;
    // Who the last shot with people to choose between ended on.
    let mut last: Option<u8> = None;
    for (start, end) in shots {
        let in_shot: Vec<(Us, &TrackedFrame)> = (timed.iter())
            .filter(|(t, _)| (start..end).contains(t))
            .copied()
            .collect();
        let plain: Vec<TrackedFrame> = in_shot.iter().map(|(_, f)| (*f).clone()).collect();
        let ranked = people_in(&plain, area);
        let cast = Cast {
            frames: &in_shot,
            ranked: &ranked,
            crop,
            area,
        };
        if let Some(plan) = cast.plan((start, end), input.talk, input.keep, last) {
            last = plan.last;
            found = true;
            // Each leg starts with a cut: the shot's start, or a switch to someone far away.
            let all = cast.aims(&plan.targets);
            for &(from, to) in &plan.legs {
                let own: Vec<(Us, f64, f64)> = all
                    .iter()
                    .copied()
                    .filter(|a| (from..to).contains(&a.0))
                    .collect();
                let aims = if own.is_empty() { &all } else { &own };
                let first = keys.len();
                if aims.is_empty() {
                    keys.push(key(from, middle(area)));
                } else {
                    follow(aims, (from, to), (crop, area), key_step, &mut keys);
                }
                if first > 0 {
                    keys[first].cut = true;
                }
            }
            continue;
        }
        let aims = aims(&in_shot, crop, area);
        found |= !aims.is_empty();
        let first = keys.len();
        if aims.is_empty() {
            keys.push(key(start, middle(area)));
        } else {
            follow(&aims, (start, end), (crop, area), key_step, &mut keys);
        }
        if first > 0 {
            keys[first].cut = true;
        }
    }
    thin(&mut keys);
    found.then_some(FocusPath { keys })
}

/// `keys` within the path's cap: the budget above counts shots, but far switches add legs of their own, so a very
/// long, busy talk can go over. Every other key that isn't a cut goes until it fits; the cuts all stay.
fn thin(keys: &mut Vec<FocusKey>) {
    while keys.len() > FocusPath::MAX_KEYS {
        let before = keys.len();
        let mut keep = false;
        keys.retain(|k| {
            keep = k.cut || !keep;
            keep
        });
        if keys.len() == before {
            break;
        }
    }
}

/// The ids of a shot's people, left to right: its main subject and those framed with them (`companions`), at most
/// `MAX_PEOPLE` (the largest). None when nobody is there.
pub fn people(frames: &[TrackedFrame]) -> Vec<u32> {
    people_in(frames, FULL)
}

/// [`people`] where the clip's crop keeps `area`: its main subject picked near the area's middle.
pub fn people_in(frames: &[TrackedFrame], area: BoxN) -> Vec<u32> {
    let Some(main) = main_subject(frames, area) else {
        return Vec::new();
    };
    let stats = stats(frames);
    let size = |id: u32| (stats.iter().find(|s| s.id == id)).map_or(0.0, |s| s.area / s.n as f64);
    let mut others = companions(frames, main);
    others.sort_by(|a, b| size(*b).total_cmp(&size(*a)).then(a.cmp(b)));
    others.truncate(usize::from(MAX_PEOPLE) - 1);
    let mut ids = vec![main];
    ids.extend(others);
    let x = |id: u32| (stats.iter().find(|s| s.id == id)).map_or(0.5, |s| s.x / s.n as f64);
    ids.sort_by(|a, b| x(*a).total_cmp(&x(*b)).then(a.cmp(b)));
    ids
}

/// Each id's sure boxes over `frames`: how many, and the sums of their areas and middles across.
struct Stat {
    id: u32,
    n: usize,
    area: f64,
    x: f64,
}

fn stats(frames: &[TrackedFrame]) -> Vec<Stat> {
    let mut out: Vec<Stat> = Vec::new();
    for b in frames
        .iter()
        .flat_map(|f| f.boxes.iter())
        .filter(|b| sure(b))
    {
        let (area, x) = (b.width() * b.height(), b.centre_x());
        match out.iter_mut().find(|s| s.id == b.id) {
            Some(s) => {
                s.n += 1;
                s.area += area;
                s.x += x;
            }
            None => out.push(Stat {
                id: b.id,
                n: 1,
                area,
                x,
            }),
        }
    }
    out
}

/// The box around the sure boxes of `ids` on `frame`, if any is there.
pub fn group_box(frame: &TrackedFrame, ids: &[u32]) -> Option<TrackedBox> {
    (frame.boxes.iter())
        .filter(|b| ids.contains(&b.id) && sure(b))
        .copied()
        .reduce(|a, b| TrackedBox {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
            ..a
        })
}

/// Whether `ids` fit together in a crop `crop_w` wide: on at least half the frames where two or more of them are
/// there (true when that is never).
pub fn fit_together(frames: &[TrackedFrame], ids: &[u32], crop_w: f64) -> bool {
    let (mut together, mut fit) = (0, 0);
    for f in frames {
        let there = (f.boxes.iter())
            .filter(|b| ids.contains(&b.id) && sure(b))
            .count();
        if there < 2 {
            continue;
        }
        together += 1;
        if group_box(f, ids).is_some_and(|g| g.width() <= crop_w * FIT) {
            fit += 1;
        }
    }
    fit * 2 >= together
}

/// Who the crop is on: one person or several, by their number from the left (from 1).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    One(u8),
    Group(Vec<u8>),
}

/// A shot's plan where it chooses between people: who it frames from when (`targets`, the first at its start), the
/// stretches between its cuts (`legs`), and who it ends on.
struct Plan {
    targets: Vec<(Us, Target)>,
    legs: Vec<(Us, Us)>,
    last: Option<u8>,
}

/// A shot's frames and its people, for a crop.
struct Cast<'a> {
    frames: &'a [(Us, &'a TrackedFrame)],
    ranked: &'a [u32],
    crop: (f64, f64),
    area: BoxN,
}

/// One stretch of speech that may take the frame: from, to, and who wants it (one person, or several talking).
struct Claim {
    from: Us,
    to: Us,
    who: Vec<u8>,
    /// Each one's talk in it.
    said: Vec<(u8, Us)>,
}

impl Cast<'_> {
    fn plain(&self) -> Vec<TrackedFrame> {
        self.frames.iter().map(|(_, f)| (*f).clone()).collect()
    }

    fn ids(&self, who: &[u8]) -> Vec<u32> {
        (who.iter())
            .filter_map(|n| self.ranked.get(usize::from(*n).checked_sub(1)?).copied())
            .collect()
    }

    fn fits(&self, who: &[u8]) -> bool {
        fit_together(&self.plain(), &self.ids(who), self.crop.0)
    }

    fn everyone(&self) -> Vec<u8> {
        (1..=self.ranked.len() as u8).collect()
    }

    /// The shot's plan, or None where it is framed as before: fewer than two people, or whoever speaks with nobody
    /// heard or everyone fitting.
    fn plan(
        &self,
        (start, end): (Us, Us),
        talk: &[Talk],
        keep: &Keep,
        last: Option<u8>,
    ) -> Option<Plan> {
        let n = self.ranked.len() as u8;
        if n < 2 {
            return None;
        }
        let talk: Vec<Talk> = (talk.iter())
            .filter(|t| t.person >= 1 && t.person <= n && t.start_us < end && start < t.end_us)
            .map(|t| Talk {
                start_us: t.start_us.max(start),
                end_us: t.end_us.min(end),
                person: t.person,
            })
            .collect();
        let all_fit = self.fits(&self.everyone());
        let spans = spans(keep, (start, end));
        let speaker_only = spans
            .iter()
            .all(|(_, _, w)| matches!(self.who(*w), Who::Speaker));
        if speaker_only && (talk.is_empty() || all_fit) {
            return None;
        }
        let main = main_subject(&self.plain(), self.area)
            .and_then(|id| self.ranked.iter().position(|r| *r == id))
            .map_or(1, |i| i as u8 + 1);
        let mut targets: Vec<(Us, Target)> = Vec::new();
        let mut cur: Option<Target> = None;
        let mut last_switch = start;
        for (from, to, who) in spans {
            let within: Vec<Talk> = (talk.iter())
                .filter(|t| t.start_us < to && from < t.end_us)
                .map(|t| Talk {
                    start_us: t.start_us.max(from),
                    end_us: t.end_us.min(to),
                    person: t.person,
                })
                .collect();
            match self.who(who) {
                Who::Person { person } => switch(&mut targets, &mut cur, from, Target::One(person)),
                Who::Both if all_fit => {
                    switch(&mut targets, &mut cur, from, Target::Group(self.everyone()))
                }
                Who::Speaker if all_fit => {
                    switch(&mut targets, &mut cur, from, Target::Group(self.everyone()))
                }
                Who::Both => {
                    let said = said(&within, (from, to));
                    let now = cur.clone().or_else(|| last.map(Target::One));
                    let pick = most(&said, now.as_ref(), main);
                    switch(&mut targets, &mut cur, from, Target::One(pick));
                }
                Who::Speaker => {
                    let claims = claims(&within);
                    if cur.is_none() {
                        // The shot opens on its first speaker, else the last one, else its main subject.
                        let open = match claims.first() {
                            Some(c) => self.resolve(c, None, main),
                            None => Target::One(last.filter(|l| *l <= n).unwrap_or(main)),
                        };
                        switch(&mut targets, &mut cur, from, open);
                    } else if targets.last().is_some_and(|(at, _)| *at < from) {
                        // An explicit range before: the speaker's rules start over here.
                        last_switch = last_switch.max(from - MIN_HOLD_US);
                    }
                    for c in &claims {
                        let target = self.resolve(c, cur.as_ref(), main);
                        if cur.as_ref() == Some(&target) {
                            continue;
                        }
                        let at = c.from.max(last_switch + MIN_HOLD_US);
                        if c.to - at < MIN_LEFT_US.min(c.to - c.from) || at >= c.to {
                            continue;
                        }
                        switch(&mut targets, &mut cur, at, target);
                        last_switch = at;
                    }
                }
            }
        }
        if targets.is_empty() {
            targets.push((start, Target::One(main)));
        }
        targets[0].0 = start;
        // A switch to someone far away cuts there; a near one eases within the leg.
        let mut legs = Vec::new();
        let mut leg_start = start;
        for w in targets.windows(2) {
            let at = w[1].0;
            let before = self.x_near(&w[0].1, at);
            let after = self.x_near(&w[1].1, at);
            if let (Some(a), Some(b)) = (before, after)
                && (a - b).abs() > CUT_AWAY * self.crop.0
            {
                legs.push((leg_start, at));
                leg_start = at;
            }
        }
        legs.push((leg_start, end));
        let last = match targets.last().map(|t| &t.1) {
            Some(Target::One(p)) => Some(*p),
            _ => last,
        };
        Some(Plan {
            targets,
            legs,
            last,
        })
    }

    /// `w` as this shot can keep it: a person who isn't there is whoever speaks.
    fn who(&self, w: Who) -> Who {
        match w {
            Who::Person { person } if person == 0 || usize::from(person) > self.ranked.len() => {
                Who::Speaker
            }
            w => w,
        }
    }

    /// Who a claim frames: its one speaker; or, for several, all of them where they fit, else whoever said more
    /// (the one framed now on a tie).
    fn resolve(&self, c: &Claim, now: Option<&Target>, main: u8) -> Target {
        match c.who.as_slice() {
            [one] => Target::One(*one),
            several if self.fits(several) => Target::Group(several.to_vec()),
            _ => Target::One(most(&c.said, now, main)),
        }
    }

    /// The aim on each frame for who is framed then: (time, x, y).
    fn aims(&self, targets: &[(Us, Target)]) -> Vec<(Us, f64, f64)> {
        let mut out = Vec::new();
        for (t, frame) in self.frames {
            let n = targets.partition_point(|(at, _)| at <= t);
            let target = &targets[n.saturating_sub(1)].1;
            let who = match target {
                Target::One(p) => vec![*p],
                Target::Group(ps) => ps.clone(),
            };
            let Some(group) = group_box(frame, &self.ids(&who)) else {
                continue;
            };
            let crop = self.crop;
            let y = if crop.1 >= self.area.h * 0.999 {
                middle(self.area).1
            } else {
                group.top - HEADROOM * crop.1 + crop.1 / 2.0
            };
            out.push((*t, group.centre_x(), y));
        }
        out
    }

    /// Where `target` is across near `at`: on the nearest frame that has it.
    fn x_near(&self, target: &Target, at: Us) -> Option<f64> {
        let who = match target {
            Target::One(p) => vec![*p],
            Target::Group(ps) => ps.clone(),
        };
        let ids = self.ids(&who);
        (self.frames.iter())
            .filter_map(|(t, f)| Some(((t - at).abs(), group_box(f, &ids)?.centre_x())))
            .min_by_key(|(d, _)| *d)
            .map(|(_, x)| x)
    }
}

/// `t` framed from `at`, unless it is framed already.
fn switch(targets: &mut Vec<(Us, Target)>, cur: &mut Option<Target>, at: Us, t: Target) {
    if cur.as_ref() != Some(&t) {
        targets.push((at, t.clone()));
        *cur = Some(t);
    }
}

/// `keep` over a shot: (from, to, who) in order, covering it. A stretch no range covers that is shorter than
/// `MIN_TALK_US` goes with the range after it (at the shot's end, the one before): chat's ranges are its timeline's
/// tenths of a second, so a part's first or last moments can fall outside them, and a stretch too short for anyone's
/// talk to count would open the shot on its main subject, held `MIN_HOLD_US` before the speaker takes the frame. So
/// does a range that covers only that much of the shot's start or end (one that ends or starts just inside it): it
/// would show its person for a moment before the next stretch takes the frame. Neighbours that keep alike are one
/// stretch.
fn spans(keep: &Keep, (start, end): (Us, Us)) -> Vec<(Us, Us, Who)> {
    let Keep::Ranges { ranges } = keep else {
        return vec![(start, end, keep.who_at(start))];
    };
    let mut out: Vec<(Us, Us, Who)> = Vec::new();
    let mut at = start;
    for r in ranges
        .iter()
        .filter(|r| r.start_us < end && start < r.end_us)
    {
        let (mut from, to) = (r.start_us.max(start), r.end_us.min(end));
        if from > at {
            if from - at < MIN_TALK_US {
                from = at;
            } else {
                out.push((at, from, Who::Speaker));
            }
        }
        out.push((from, to, r.who));
        at = to;
    }
    if at < end {
        match out.last_mut() {
            Some(last) if end - at < MIN_TALK_US => last.1 = end,
            _ => out.push((at, end, Who::Speaker)),
        }
    }
    // A sliver at either edge goes with its neighbour.
    while out.len() > 1 && out[0].1 - out[0].0 < MIN_TALK_US {
        out[1].0 = out[0].0;
        out.remove(0);
    }
    while out.len() > 1 && out[out.len() - 1].1 - out[out.len() - 1].0 < MIN_TALK_US {
        let last = out.pop().expect("two or more");
        out.last_mut().expect("one or more").1 = last.1;
    }
    out.dedup_by(|later, earlier| {
        let alike = earlier.2 == later.2 && earlier.1 == later.0;
        if alike {
            earlier.1 = later.1;
        }
        alike
    });
    out
}

/// Each person's talk in `talk` within `(from, to)`, by person.
fn said(talk: &[Talk], (from, to): (Us, Us)) -> Vec<(u8, Us)> {
    let mut out: Vec<(u8, Us)> = Vec::new();
    for t in talk {
        let len = (t.end_us.min(to) - t.start_us.max(from)).max(0);
        match out.iter_mut().find(|(p, _)| *p == t.person) {
            Some((_, sum)) => *sum += len,
            None => out.push((t.person, len)),
        }
    }
    out
}

/// Who said the most in `said`: the one framed `now` on a tie, else the first to speak; `main` when nobody did.
fn most(said: &[(u8, Us)], now: Option<&Target>, main: u8) -> u8 {
    let top = said.iter().map(|(_, len)| *len).max().unwrap_or(0);
    if top == 0 {
        return match now {
            Some(Target::One(p)) => *p,
            _ => main,
        };
    }
    let tied: Vec<u8> = (said.iter())
        .filter(|(_, len)| *len == top)
        .map(|(p, _)| *p)
        .collect();
    match now {
        Some(Target::One(p)) if tied.contains(p) => *p,
        _ => tied[0],
    }
}

/// The stretches of `talk` that may take the frame, in order: one person's run of at least `MIN_TALK_US` alone, and
/// busy stretches (short turns back and forth, or talk over each other) at least that long, of two or more people who
/// each say `MIN_SHARE_US` in it, or of one who says `MIN_TALK_US` in short turns.
fn claims(talk: &[Talk]) -> Vec<Claim> {
    // Each person's talk joined across short gaps.
    let mut runs: Vec<Talk> = talk.to_vec();
    runs.sort_by_key(|t| (t.person, t.start_us));
    let mut joined: Vec<Talk> = Vec::new();
    for t in runs {
        match joined.last_mut() {
            Some(l) if l.person == t.person && t.start_us <= l.end_us + RUN_GAP_US => {
                l.end_us = l.end_us.max(t.end_us);
            }
            _ => joined.push(t),
        }
    }
    // Pieces between every edge, each with who talks there.
    let mut edges: Vec<Us> = joined.iter().flat_map(|t| [t.start_us, t.end_us]).collect();
    edges.sort_unstable();
    edges.dedup();
    let mut pieces: Vec<(Us, Us, Vec<u8>)> = Vec::new();
    for w in edges.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut who: Vec<u8> = (joined.iter())
            .filter(|t| t.start_us <= a && b <= t.end_us)
            .map(|t| t.person)
            .collect();
        who.sort_unstable();
        who.dedup();
        if who.is_empty() {
            continue;
        }
        match pieces.last_mut() {
            Some(l) if l.1 == a && l.2 == who && who.len() == 1 => l.1 = b,
            _ => pieces.push((a, b, who)),
        }
    }
    let mut claims = Vec::new();
    let mut busy: Vec<(Us, Us, Vec<u8>)> = Vec::new();
    let flush = |busy: &mut Vec<(Us, Us, Vec<u8>)>, claims: &mut Vec<Claim>| {
        if let (Some(first), Some(last)) = (busy.first(), busy.last()) {
            let (from, to) = (first.0, last.1);
            let said = said(talk, (from, to));
            // Who takes part: an "mm-hm" over someone's short turns doesn't make the stretch theirs too.
            let who: Vec<u8> = (said.iter())
                .filter(|(_, len)| *len >= MIN_SHARE_US)
                .map(|(p, _)| *p)
                .collect();
            let alone_enough = who.len() == 1 && said.iter().any(|(_, len)| *len >= MIN_TALK_US);
            if to - from >= MIN_TALK_US && (who.len() >= 2 || alone_enough) {
                let mut who = who;
                who.sort_unstable();
                claims.push(Claim {
                    from,
                    to,
                    said,
                    who,
                });
            }
        }
        busy.clear();
    };
    for piece in pieces {
        let (a, b, who) = &piece;
        if who.len() == 1 && b - a >= MIN_TALK_US {
            flush(&mut busy, &mut claims);
            claims.push(Claim {
                from: *a,
                to: *b,
                who: who.clone(),
                said: vec![(who[0], b - a)],
            });
            continue;
        }
        if busy.last().is_some_and(|l| *a > l.1 + BUSY_GAP_US) {
            flush(&mut busy, &mut claims);
        }
        busy.push(piece);
    }
    flush(&mut busy, &mut claims);
    claims.sort_by_key(|c| c.from);
    claims
}

/// The id of the shot's main subject in `frames`: the most area near the middle of `area` across the frames (a box's
/// area times how near its middle is to `area`'s, from 1 there to 0.5 at its edge). None when nobody is there.
pub fn main_subject(frames: &[TrackedFrame], area: BoxN) -> Option<u32> {
    let mid = area.x + area.w / 2.0;
    let mut scores: Vec<(u32, f64)> = Vec::new();
    for b in frames
        .iter()
        .flat_map(|f| f.boxes.iter())
        .filter(|b| sure(b))
    {
        let near = (1.0 - (b.centre_x() - mid).abs() / area.w).max(0.0);
        let score = b.width() * b.height() * near;
        match scores.iter_mut().find(|(id, _)| *id == b.id) {
            Some((_, s)) => *s += score,
            None => scores.push((b.id, score)),
        }
    }
    scores
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(id, _)| id)
}

fn sure(b: &TrackedBox) -> bool {
    b.confidence.is_none_or(|c| c >= MIN_CONFIDENCE) && b.width() > 0.0 && b.height() > 0.0
}

/// `area` when it is a part of the picture with some size, else the whole picture.
fn sane_area(area: BoxN) -> BoxN {
    let ok = [area.x, area.y, area.w, area.h]
        .iter()
        .all(|v| v.is_finite())
        && area.w > 1e-3
        && area.h > 1e-3
        && area.x >= 0.0
        && area.y >= 0.0
        && area.x + area.w <= 1.0 + 1e-9
        && area.y + area.h <= 1.0 + 1e-9;
    if ok { area } else { FULL }
}

fn middle(area: BoxN) -> (f64, f64) {
    (area.x + area.w / 2.0, area.y + area.h / 2.0)
}

fn sane_side(side: f64) -> f64 {
    if side.is_finite() {
        side.clamp(1e-3, 1.0)
    } else {
        1.0
    }
}

/// The shots between `start` and `end`, split at the cuts strictly inside.
pub fn shots(start: Us, end: Us, cuts: &[Us]) -> Vec<(Us, Us)> {
    let mut edges = vec![start];
    edges.extend(cuts.iter().copied().filter(|c| *c > start && *c < end));
    edges.sort_unstable();
    edges.dedup();
    edges.push(end);
    edges.windows(2).map(|w| (w[0], w[1])).collect()
}

/// The aim on each frame of a shot where its subject (and anyone framed with them) is: (time, x, y).
fn aims(frames: &[(Us, &TrackedFrame)], crop: (f64, f64), area: BoxN) -> Vec<(Us, f64, f64)> {
    let plain: Vec<TrackedFrame> = frames.iter().map(|(_, f)| (*f).clone()).collect();
    let Some(main) = main_subject(&plain, area) else {
        return Vec::new();
    };
    let together = companions(&plain, main);
    let mut out = Vec::new();
    for (t, frame) in frames {
        let Some(subject) = frame.boxes.iter().find(|b| b.id == main && sure(b)) else {
            continue;
        };
        let mut group = *subject;
        for other in frame
            .boxes
            .iter()
            .filter(|b| together.contains(&b.id) && sure(b))
        {
            let joined = TrackedBox {
                left: group.left.min(other.left),
                top: group.top.min(other.top),
                right: group.right.max(other.right),
                bottom: group.bottom.max(other.bottom),
                ..group
            };
            // Only while both fit in the crop, with a little room.
            if joined.width() <= crop.0 * 0.9 {
                group = joined;
            }
        }
        let x = group.centre_x();
        let y = if crop.1 >= area.h * 0.999 {
            middle(area).1
        } else {
            group.top - HEADROOM * crop.1 + crop.1 / 2.0
        };
        out.push((*t, x, y));
    }
    out
}

/// Who is framed with `main`: there on at least half the frames, and at least half its size on average.
fn companions(frames: &[TrackedFrame], main: u32) -> Vec<u32> {
    let mut seen: Vec<(u32, usize, f64)> = Vec::new();
    for b in frames
        .iter()
        .flat_map(|f| f.boxes.iter())
        .filter(|b| sure(b))
    {
        let area = b.width() * b.height();
        match seen.iter_mut().find(|(id, _, _)| *id == b.id) {
            Some((_, n, sum)) => {
                *n += 1;
                *sum += area;
            }
            None => seen.push((b.id, 1, area)),
        }
    }
    let mean = |id: u32| {
        (seen.iter().find(|(i, _, _)| *i == id)).map_or(0.0, |(_, n, sum)| sum / *n as f64)
    };
    let main_area = mean(main);
    seen.iter()
        .filter(|(id, n, sum)| {
            *id != main && *n * 2 >= frames.len() && sum / *n as f64 >= main_area * 0.5
        })
        .map(|(id, _, _)| *id)
        .collect()
}

/// The aim at `t`: straight between two frames close enough, else the nearest frame's before (or the first's).
fn aim_at(aims: &[(Us, f64, f64)], t: Us) -> (f64, f64) {
    let n = aims.partition_point(|a| a.0 <= t);
    if n == 0 {
        return (aims[0].1, aims[0].2);
    }
    let a = aims[n - 1];
    match aims.get(n) {
        Some(b) if b.0 - a.0 <= MAX_GAP_US && b.0 > a.0 => {
            let k = (t - a.0) as f64 / (b.0 - a.0) as f64;
            (a.1 + (b.1 - a.1) * k, a.2 + (b.2 - a.2) * k)
        }
        _ => (a.1, a.2),
    }
}

/// One axis of the crop as it follows, inside `lo` to `lo + room`.
struct Axis {
    c: f64,
    v: f64,
    size: f64,
    lo: f64,
    room: f64,
    /// Moving toward the aim (+1 or -1), or at rest (0).
    moving: f64,
}

impl Axis {
    /// The crop's centre kept where the crop stays inside its area.
    fn inside(&self, c: f64) -> f64 {
        let half = self.size / 2.0;
        let lo = self.lo + half;
        c.clamp(lo, (self.lo + self.room - half).max(lo))
    }

    fn step(&mut self, aim: f64, dt: f64) {
        let aim = self.inside(aim);
        let off = aim - self.c;
        if self.moving == 0.0 {
            if off.abs() <= self.size * DEAD_ZONE / 2.0 {
                return;
            }
            self.moving = off.signum();
        }
        // Never back: an aim behind the move ends it where it is.
        let want = if off * self.moving < 0.0 { self.c } else { aim };
        let (mut next, mut v) = step(self.c, self.v, want, OMEGA, dt);
        let most = SPEED * self.size * dt;
        let moved = (next - self.c).clamp(-most, most);
        if moved * self.moving < 0.0 {
            (next, v) = (self.c, 0.0);
        } else {
            next = self.c + moved;
            v = v.clamp(-SPEED * self.size, SPEED * self.size);
        }
        self.c = self.inside(next);
        self.v = v;
        if (aim - self.c).abs() <= self.size * SETTLE || off * self.moving < 0.0 {
            self.moving = 0.0;
            self.v = 0.0;
        }
    }
}

/// The keys of one shot with a subject, from `start` (its first key) every `key_step` until `end`.
fn follow(
    aims: &[(Us, f64, f64)],
    (start, end): (Us, Us),
    (crop, area): ((f64, f64), BoxN),
    key_step: Us,
    keys: &mut Vec<FocusKey>,
) {
    let early: Vec<&(Us, f64, f64)> = (aims.iter())
        .filter(|a| (start..start + START_US).contains(&a.0))
        .collect();
    let opening = if early.is_empty() {
        (aims[0].1, aims[0].2)
    } else {
        (
            median(early.iter().map(|a| a.1)),
            median(early.iter().map(|a| a.2)),
        )
    };
    let mut x = Axis {
        c: 0.5,
        v: 0.0,
        size: crop.0,
        lo: area.x,
        room: area.w,
        moving: 0.0,
    };
    let mut y = Axis {
        c: 0.5,
        v: 0.0,
        size: crop.1,
        lo: area.y,
        room: area.h,
        moving: 0.0,
    };
    x.c = x.inside(opening.0);
    y.c = y.inside(opening.1);
    let dt = DT_US as f64 / 1e6;
    let (mut t, mut next_key) = (start, start);
    while next_key < end {
        while t < next_key {
            let aim = aim_at(aims, t);
            x.step(aim.0, dt);
            y.step(aim.1, dt);
            t += DT_US;
        }
        keys.push(key(next_key, (x.c, y.c)));
        next_key += key_step;
    }
    let first = keys.len() - shot_keys(keys, start);
    drop_holds(keys, first);
}

/// How many of `keys` are this shot's: from `start` on.
fn shot_keys(keys: &[FocusKey], start: Us) -> usize {
    keys.iter().rev().take_while(|k| k.at_us >= start).count()
}

/// Keeps only the first and last key of each run of equal keys from `first` on: the centre goes straight between
/// them, so the crop draws the same.
fn drop_holds(keys: &mut Vec<FocusKey>, first: usize) {
    let same = |a: &FocusKey, b: &FocusKey| a.x == b.x && a.y == b.y;
    let shot: Vec<FocusKey> = keys.split_off(first);
    let n = shot.len();
    for i in 0..n {
        let inside =
            i > 0 && i + 1 < n && same(&shot[i - 1], &shot[i]) && same(&shot[i], &shot[i + 1]);
        if !inside {
            keys.push(shot[i]);
        }
    }
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn key(at_us: Us, (x, y): (f64, f64)) -> FocusKey {
    FocusKey {
        at_us,
        x: x.clamp(0.0, 1.0),
        y: y.clamp(0.0, 1.0),
        cut: false,
    }
}
