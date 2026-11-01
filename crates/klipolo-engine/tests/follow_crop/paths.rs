//! How a take keeps its follow-crop paths: built from the raw track whatever the style's smoothing, built again when
//! its screen activity grows (the key carries it), the old one kept by a context made before, and never more paths than
//! a take holds.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use klipolo_cursor::activity::{ActivityTrack, Region, Tick, decode, encode_chunk, header};
use klipolo_engine::media::cursor::{CursorTracks, FollowKey, MAX_FOLLOWS, wanted};
use klipolo_engine::plan::PlanContext;
use klipolo_model::cursor_style::{CursorStyle, Follow, Smoothing};
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::{Framing, FramingMode};

use super::support::secs;
use super::{exported, resting_at, take};

/// 60 s of a screen file whose chunk is made (`made`) or not: lines typed low down, right of the middle (a glyph a
/// tick, a new line every 3 s) from 20 s to 50 s, the screen still otherwise.
fn screen_activity(made: bool) -> ActivityTrack {
    let mut bytes = Vec::new();
    header(secs(60.0), &mut bytes);
    if made {
        let ticks: Vec<Tick> = (0..600_u32)
            .map(|k| {
                let x = 30 + (k.saturating_sub(200) % 30) as u8;
                let glyph = Region {
                    x0: x,
                    y0: 30,
                    x1: x,
                    y1: 32,
                    cells: 3,
                };
                match (200..500).contains(&k) {
                    true => Tick {
                        changed: 3,
                        top: [glyph, Region::default()],
                    },
                    false => Tick::default(),
                }
            })
            .collect();
        encode_chunk(0, &ticks, &mut bytes);
    }
    decode(&bytes, secs(60.0))
}

type Bits = Option<(u64, u64)>;

fn bits(c: Option<(f64, f64)>) -> Bits {
    c.map(|c| (c.0.to_bits(), c.1.to_bits()))
}

#[test]
fn the_follow_path_ignores_the_style_smoothing_and_rebuilds_when_activity_completes() {
    let dir = tempfile::tempdir().unwrap();
    let style = |smoothing| CursorStyle {
        smoothing,
        ..CursorStyle::default()
    };
    let lines = resting_at(0.2, 0.5);
    let off = take(dir.path(), &lines, style(Smoothing::Off));
    let strong = take(dir.path(), &lines, style(Smoothing::Strong));
    let (a, b) = (exported(&off), exported(&strong));
    let want = &wanted(&off)[0];
    assert_eq!(want.crops, wanted(&strong)[0].crops, "the same crops");
    let take_b = b.takes.get(&want.path).unwrap();
    assert_eq!(take_b.variants(), 1, "Strong has its smoothed path");
    let follow =
        |ctx: &PlanContext, t| bits(ctx.follows.get(&want.path, want.crops[0])?.centre_at(t));
    for t in (0..secs(60.0)).step_by(100_000) {
        assert!(follow(&a, t).is_some());
        assert_eq!(follow(&a, t), follow(&b, t), "at {t}");
    }
    // The screen file's activity: none made yet, then its chunk made (typing from 20 s).
    let current = Arc::new(Mutex::new(Arc::new(screen_activity(false))));
    let tracks = Arc::new(CursorTracks::default());
    let (seen, screen) = (current.clone(), want.screens[0].1.clone());
    let of = move |p: &str| (p == screen).then(|| seen.lock().unwrap().clone());
    tracks.set_activity(Arc::new(of));
    let wants = wanted(&off);
    tracks.want(&wants);
    assert!(tracks.wait_ready(&wants, Duration::from_secs(10)));
    let take = tracks.get(&want.path).unwrap();
    let before = take.activity();
    let key = |print| FollowKey::new(want.crops[0], print);
    let early = take.follow(key(before)).expect("built for what is made");
    assert_eq!(
        take.facts().typing.as_deref(),
        Some(&[][..]),
        "no typing known yet"
    );
    let (playing, _) = PlanContext::starting(Arc::new(off.clone()), None, &tracks);
    *current.lock().unwrap() = Arc::new(screen_activity(true));
    tracks.want(&wants);
    assert!(tracks.wait_ready(&wants, Duration::from_secs(10)));
    let after = take.activity();
    assert_ne!(before, after, "the key carries the activity");
    assert!(take.follow(key(before)).is_none(), "the old path went");
    let late = take.follow(key(after)).expect("built again");
    assert!(
        take.facts().typing.as_ref().is_some_and(|t| !t.is_empty()),
        "the typing is known"
    );
    let differ = (0..secs(60.0))
        .step_by(100_000)
        .any(|t| early.centre_at(t) != late.centre_at(t));
    assert!(differ, "the typing moves the crop");
    // A context made before keeps the path it drew with: the session takes the new one at the next pause or seek, so
    // nothing jumps mid-play. An edit while playing keeps the takes and paths as they were too;
    // one made paused takes the new.
    let kept = playing.follows.get(&want.path, want.crops[0]).unwrap();
    let project = Arc::new(off.clone());
    let (held, _) = PlanContext::starting_held(project.clone(), &playing, &tracks);
    let (paused, _) = PlanContext::starting(project, Some(&playing), &tracks);
    assert!(
        Arc::ptr_eq(&held.takes, &playing.takes) && Arc::ptr_eq(&held.follows, &playing.follows)
    );
    let edited = held.follows.get(&want.path, want.crops[0]).unwrap();
    let fresh = paused.follows.get(&want.path, want.crops[0]).unwrap();
    for t in (0..secs(60.0)).step_by(100_000) {
        assert_eq!(bits(kept.centre_at(t)), bits(early.centre_at(t)), "at {t}");
        assert_eq!(
            bits(edited.centre_at(t)),
            bits(early.centre_at(t)),
            "edited at {t}"
        );
        assert_eq!(
            bits(fresh.centre_at(t)),
            bits(late.centre_at(t)),
            "paused at {t}"
        );
    }
}

#[test]
fn a_take_never_wants_more_follow_paths_than_it_holds() {
    // Three clips of one take following at three distances, in portrait and square: six crops, of which
    // the first `MAX_FOLLOWS` in timeline order are built, and the take is ready (never built again on every publish).
    // A clip with a crop past them shows its Fill crop at its focus.
    let dir = tempfile::tempdir().unwrap();
    let mut p = take(dir.path(), &resting_at(0.3, 0.5), CursorStyle::default());
    let first = p.timeline.main[0].clone();
    p.timeline.main = [1.0, 1.5, 2.0]
        .into_iter()
        .map(|closer| {
            let mut c = first.clone();
            for format in [FormatKind::Portrait, FormatKind::Square] {
                let framing = Framing {
                    mode: FramingMode::Fill,
                    focus_x: 0.5,
                    focus_y: 0.5,
                    follow: Some(Follow { closer }),
                };
                c.framing.insert(format, framing);
            }
            c
        })
        .collect();
    let wants = wanted(&p);
    assert_eq!(wants[0].crops.len(), MAX_FOLLOWS);
    let tracks = CursorTracks::default();
    tracks.want(&wants);
    assert!(tracks.wait_ready(&wants, Duration::from_secs(10)));
    let take = tracks.get(&wants[0].path).unwrap();
    assert_eq!(take.follows().len(), MAX_FOLLOWS);
    assert!(
        take.ready_for(&wants[0], take.activity()),
        "nothing to build again"
    );
}

/// A minute made past one that is missing (passed over as unreadable) changes the activity's
/// fingerprint though how far the file is whole doesn't, so the plans keyed on it are made again.
#[test]
fn a_minute_made_past_a_gap_changes_the_activity_fingerprint() {
    use klipolo_engine::media::cursor::{ActivityOf, Screens};
    let minutes = |made: &[u32]| {
        let mut bytes = Vec::new();
        header(secs(180.0), &mut bytes);
        for &c in made {
            encode_chunk(c, &vec![Tick::default(); 600], &mut bytes);
        }
        Arc::new(decode(&bytes, secs(180.0)))
    };
    let print = |track: Arc<ActivityTrack>| {
        let of: ActivityOf = Arc::new(move |_: &str| Some(track.clone()));
        Screens::read(&[(0, "/media/screen".to_string())], Some(&of)).print
    };
    let (first, past) = (minutes(&[0]), minutes(&[0, 2]));
    assert_eq!(first.complete_through(), past.complete_through());
    assert_ne!(print(first), print(past));
}

#[test]
#[ignore = "real scale: a two-hour take's facts and follow paths, in release"]
fn two_hours_of_follow_paths_build_off_the_actor_in_well_under_a_second() {
    use klipolo_cursor::activity::CHUNK_TICKS;
    use klipolo_cursor::format::{Capture, Line};
    use klipolo_engine::media::cursor::{Screens, load_take};
    // Two hours: the cursor wandering at 30 Hz (resting 3 s every 10), its screen typing a line every minute while it rests.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cursor.jsonl");
    let mut lines = vec![super::support::takes::header(Capture::Separate)];
    lines.extend((0..7_200 * 30_i64).map(|i| {
        let a = i as f64 / 30.0;
        let still = (a % 10.0) < 3.0;
        let (x, y) = if still {
            (0.3, 0.6)
        } else {
            (0.5 + 0.3 * (a * 0.7).sin(), 0.5 + 0.3 * (a * 0.4).cos())
        };
        Line::Move {
            t: i * 33_333,
            x: x as f32,
            y: y as f32,
        }
    }));
    super::support::takes::write(&path, &lines);
    let mut bytes = Vec::new();
    header(secs(7_200.0), &mut bytes);
    let minute: Vec<Tick> = (0..CHUNK_TICKS)
        .map(|k| match k < 25 {
            true => Tick {
                changed: 3,
                top: [
                    Region {
                        x0: 20 + k as u8,
                        y0: 10,
                        x1: 20 + k as u8,
                        y1: 12,
                        cells: 3,
                    },
                    Region::default(),
                ],
            },
            false => Tick::default(),
        })
        .collect();
    for i in 0..120 {
        encode_chunk(i, &minute, &mut bytes);
    }
    let activity = Arc::new(decode(&bytes, secs(7_200.0)));
    assert_eq!(activity.complete_through(), secs(7_200.0));
    let screens = Screens {
        print: 3,
        held: vec![(0, activity)],
    };
    let take = load_take(&path).unwrap();
    let before = take.bytes();
    let crops = [(316, 1_000, 1_000), (563, 1_000, 1_000)];
    let started = std::time::Instant::now();
    take.prepare_follow(&crops, &screens, 16.0 / 9.0);
    let built = started.elapsed();
    let typed = take.facts().typing.as_ref().map_or(0, Vec::len);
    let paths = take.follows();
    let started = std::time::Instant::now();
    let mut sum = 0.0;
    for t in (0..secs(7_200.0)).step_by(16_667) {
        sum += paths[0].1.centre_at(t).map_or(0.0, |c| c.0);
    }
    let sampled = started.elapsed();
    println!(
        "two hours: facts ({typed} typing spans) and two follow paths in {built:?}, +{} KB in the take; 432,000 follow \
         samples in {sampled:?} ({sum:.0})",
        (take.bytes() - before) / 1024
    );
    assert_eq!(paths.len(), 2);
    assert!(typed >= 100, "{typed}");
    assert!(built.as_millis() < 500, "built in {built:?}");
}
