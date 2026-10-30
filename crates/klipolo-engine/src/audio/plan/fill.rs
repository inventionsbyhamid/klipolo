//! The camera's own sound over a recording clip's true gaps: a muted picture source whose mix
//! `fills_gaps` plays only where none of the recording's sound sources recorded anything (`Recording::true_gaps`, from
//! the runs and nudges only), at its mix's volume, effects and centring; its mute is what the fill overrides.
//! A mute is the person's choice, so a muted mic still has its runs and stays silent, and a separated mic still has its
//! runs, so the camera never doubles it. At a gap's inner edges the fill and the recorded sound cross (`cross`).

use klipolo_model::project::{Recording, RecordingSource, SourceMix};
use klipolo_model::time::{Us, unscaled};
use klipolo_model::timeline::Clip;

use super::AudioSource;

/// How long the camera's fill and the recorded sound overlap at a gap's inner edge (µs).
pub(super) const CROSS_US: Us = 10_000;
/// How far apart a fill's edge and the recorded sound's may be and still meet (µs): one sample at 48 kHz, so a
/// rounding change on either side keeps the crossfade. Both come from the same recording-time
/// boundary through the same `scaled()`, so today they are equal.
const EDGE_US: Us = 21;

/// Where a source of a recording clip is heard: everywhere it has runs, only over the clip's true gaps, or not at
/// all.
pub(super) enum Heard<'a> {
    Everywhere,
    Gaps(&'a [(Us, Us)]),
    Nowhere,
}

/// One recording clip's true gaps, worked out the first time a source fills them: a clip without a fill allocates
/// nothing.
pub(super) struct Gaps<'a> {
    rec: &'a Recording,
    clip: &'a Clip,
    found: Option<Vec<(Us, Us)>>,
}

impl<'a> Gaps<'a> {
    pub(super) fn of(rec: &'a Recording, clip: &'a Clip) -> Self {
        Self {
            rec,
            clip,
            found: None,
        }
    }

    /// Where `source` is heard in the clip at its mix there, `mix`: muted, only a picture source that fills gaps is,
    /// over them.
    pub(super) fn heard(&mut self, source: &RecordingSource, mix: &SourceMix) -> Heard<'_> {
        if mix.volume <= 0.0 {
            Heard::Nowhere
        } else if !mix.muted {
            Heard::Everywhere
        } else if mix.fills_gaps && source.role.is_video() {
            let (rec, clip) = (self.rec, self.clip);
            Heard::Gaps(
                self.found
                    .get_or_insert_with(|| rec.true_gaps(clip.source_in_us, clip.source_out_us)),
            )
        } else {
            Heard::Nowhere
        }
    }
}

/// One clip's sources (`clip`; `fills` are the camera's fill spans among them), its timeline span `[start, end)` and
/// speed: at each inner edge of a gap, where the recorded sound stops or starts, the fill reaches `CROSS_US` into it
/// (less where the clip ends sooner) with a straight fade, and that sound fades the other way over the same stretch,
/// so their gains add up to 1. Two 3 ms edge ramps used to leave a 6 ms dip to silence there. Straight
/// rather than equal power: the two are one moment lined up to the sample, so they add in phase. An edge at the clip's
/// own start or end keeps the clip's fade. O(fills × sources), nothing allocated.
pub(super) fn cross(clip: &mut [AudioSource], fills: &[usize], (start, end): (Us, Us), speed: f64) {
    for &f in fills {
        let (from, to) = (clip[f].start_us, clip[f].end_us);
        if from > start {
            let x = CROSS_US.min(from - start);
            if meet(
                clip,
                fills,
                x,
                |s| (s.end_us - from).abs() <= EDGE_US,
                |s| &mut s.fade_out_us,
            ) {
                let fill = &mut clip[f];
                fill.start_us = from - x;
                fill.source_in_us -= (unscaled(x, speed) as f64 * fill.rate).round() as Us;
                fill.fade_in_us = x;
            }
        }
        if to < end {
            let x = CROSS_US.min(end - to);
            if meet(
                clip,
                fills,
                x,
                |s| (s.start_us - to).abs() <= EDGE_US,
                |s| &mut s.fade_in_us,
            ) {
                clip[f].end_us = to + x;
                clip[f].fade_out_us = x;
            }
        }
    }
}

/// Gives each recorded (not fill) source of `clip` that `at` the edge a straight fade of at least `x` on that side
/// (`fade`); true when there is one.
fn meet(
    clip: &mut [AudioSource],
    fills: &[usize],
    x: Us,
    at: impl Fn(&AudioSource) -> bool,
    fade: impl Fn(&mut AudioSource) -> &mut Us,
) -> bool {
    let mut met = false;
    for (k, s) in clip.iter_mut().enumerate() {
        if !fills.contains(&k) && at(s) {
            let f = fade(s);
            *f = (*f).max(x);
            met = true;
        }
    }
    met
}

#[cfg(test)]
mod tests {
    use klipolo_model::project::{RecordingKind, SourceRole};
    use klipolo_model::timeline::ClipSource;

    use super::*;

    #[test]
    fn a_clip_without_a_fill_works_out_no_gaps() {
        // The plan's budget: a source that plays everywhere or not at all never computes (or allocates) the gaps.
        let source = |role, mix: SourceMix| RecordingSource {
            id: "s".into(),
            role,
            label: String::new(),
            runs: Vec::new(),
            nudge_us: 0,
            mix,
            synced: None,
        };
        let rec = Recording {
            id: "r".into(),
            kind: RecordingKind::Camera,
            sources: Vec::new(),
            primary: "s".into(),
            cursor: None,
            markers: Vec::new(),
            game: None,
        };
        let clip = Clip::new(ClipSource::Recording { id: "r".into() }, 0, 10_000_000);
        let mut gaps = Gaps::of(&rec, &clip);
        let muted = SourceMix {
            muted: true,
            ..SourceMix::default()
        };
        for (role, mix) in [
            (SourceRole::Camera, SourceMix::default()),
            (SourceRole::Camera, muted.clone()),
            (
                SourceRole::Mic,
                SourceMix {
                    fills_gaps: true,
                    ..muted
                },
            ),
        ] {
            let s = source(role, mix);
            assert!(!matches!(gaps.heard(&s, &s.mix), Heard::Gaps(_)));
        }
        assert!(gaps.found.is_none());
    }

    #[test]
    fn a_fill_meets_the_recorded_sound_a_sample_apart() {
        // An edge a rounding apart (15 µs, under one 48 kHz sample) still crosses: the fill reaches
        // 10 ms into the recorded sound on both sides, which fades the other way; 1 ms apart is no edge.
        let source = |start_us, end_us| AudioSource {
            path: String::new(),
            start_us,
            end_us,
            source_in_us: 0,
            speed: 1.0,
            rate: 1.0,
            gain: 1.0,
            fade_in_us: 0,
            fade_out_us: 0,
            crossfade: Default::default(),
            filter: None,
            centre_from: None,
            duck: None,
        };
        // The recorded sound stops at 1 s (rounded 15 µs early) and starts again at 3 s (15 µs late); the fill between.
        let mut clip = [
            source(0, 999_985),
            source(3_000_015, 5_000_000),
            source(1_000_000, 3_000_000),
        ];
        cross(&mut clip, &[2], (0, 5_000_000), 1.0);
        let fill = &clip[2];
        assert_eq!((fill.start_us, fill.end_us), (990_000, 3_010_000));
        assert_eq!((fill.fade_in_us, fill.fade_out_us), (CROSS_US, CROSS_US));
        assert_eq!(
            (clip[0].fade_out_us, clip[1].fade_in_us),
            (CROSS_US, CROSS_US)
        );
        let mut apart = [source(0, 999_000), source(1_000_000, 3_000_000)];
        cross(&mut apart, &[1], (0, 5_000_000), 1.0);
        assert_eq!((apart[1].start_us, apart[0].fade_out_us), (1_000_000, 0));
    }
}
