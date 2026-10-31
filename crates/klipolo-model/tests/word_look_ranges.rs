//! One `SetWordLook` over many ranges: chat's `styleCaptionWords`, MCP's
//! `setCaptionWordLook` and the word list style every word that takes one look with one op, applied in one pass over
//! the file's looks, never one op and one shift of the list per word.

use klipolo_model::captions::looks::{Look, LookIndex, WordLook};
use klipolo_model::ops::Op;
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::time::{RangeUs, Us};

fn colour(hex: &str) -> Look {
    Look {
        color: Some(hex.into()),
        ..Look::default()
    }
}

fn big() -> Look {
    Look {
        size: Some(2),
        ..Look::default()
    }
}

/// A project with files `v` and `w`, holding `looks`.
fn project(looks: Vec<WordLook>) -> Project {
    let mut p = Project::new("Looks");
    p.assets = ["v", "w"]
        .map(|id| Asset {
            id: id.into(),
            kind: AssetKind::Video,
            path: format!("/m/{id}.mp4"),
            name: id.into(),
            duration_us: 60_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
        })
        .to_vec();
    p.word_looks = looks.into();
    p
}

fn on(asset: &str, start_us: Us, end_us: Us, look: Look) -> WordLook {
    WordLook {
        asset_id: asset.into(),
        start_us,
        end_us,
        look,
    }
}

fn set(asset: &str, ranges: &[(Us, Us)], look: Look) -> Op {
    Op::SetWordLook {
        asset_id: asset.into(),
        ranges: ranges
            .iter()
            .map(|&(start, end)| RangeUs { start, end })
            .collect(),
        look,
    }
}

#[test]
fn one_op_sets_each_range_and_keeps_the_rest_of_each_look_it_cuts() {
    let red = colour("#FF3B30");
    let p = project(vec![
        on("v", 0, 10, red.clone()),
        on("w", 0, 10, red.clone()),
    ]);
    // Out of order, and two that touch: as one.
    let op = set("v", &[(4, 5), (1, 2), (5, 6)], big());
    let p = klipolo_model::apply_ops(&p, &[op]).unwrap();
    let spans: Vec<(&str, Us, Us, bool)> = p
        .word_looks
        .iter()
        .map(|l| (l.asset_id.as_str(), l.start_us, l.end_us, l.look == big()))
        .collect();
    assert_eq!(
        spans,
        [
            ("v", 0, 1, false),
            ("v", 1, 2, true),
            ("v", 2, 4, false),
            ("v", 4, 6, true),
            ("v", 6, 10, false),
            ("w", 0, 10, false)
        ]
    );
    // An empty look over several ranges only clears them.
    let p = klipolo_model::apply_ops(&p, &[set("v", &[(0, 3), (5, 10)], Look::default())]).unwrap();
    let left: Vec<(Us, Us)> = p
        .word_looks
        .iter()
        .filter(|l| l.asset_id == "v")
        .map(|l| (l.start_us, l.end_us))
        .collect();
    assert_eq!(left, [(3, 4), (4, 5)]);
}

#[test]
fn an_op_with_no_range_a_bad_one_or_more_than_a_project_holds_is_refused() {
    let p = project(Vec::new());
    let code = |op: Op| {
        klipolo_model::apply_ops(&p, &[op])
            .unwrap_err()
            .coded()
            .code
    };
    assert_eq!(code(set("v", &[], big())), "model.wordLook.range");
    assert_eq!(
        code(set("v", &[(1, 2), (3, 3)], big())),
        "model.wordLook.range"
    );
    let many: Vec<(Us, Us)> = (0..20_001).map(|i| (2 * i, 2 * i + 1)).collect();
    assert_eq!(code(set("v", &many, big())), "model.wordLook.tooMany");
}

/// splitmix64 (Steele, Lea and Flood): every draw comes from a counter, so the sequence never repeats in a test.
struct Mix(u64);

impl Mix {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % n
    }
}

/// Every point of the file reads, after one op over many ranges, what the plain rule says: the op's look inside any of
/// its ranges (none for an empty one), else the look that was there. The other file keeps its own, and the list stays
/// one `validate` accepts (sorted, never overlapping), whatever the ranges' order, overlaps and touches.
#[test]
fn every_point_reads_the_ops_look_inside_its_ranges_and_the_old_one_outside() {
    let mut rnd = Mix(28);
    let looks = [colour("#FF3B30"), big(), colour("#32D74B"), Look::default()];
    for case in 0..2_000 {
        // Old looks: apart or touching, over 0..100 µs of v, and one on w.
        let mut old = Vec::new();
        let mut at = rnd.below(6) as Us;
        while at < 95 {
            let end = at + 1 + rnd.below(12) as Us;
            old.push(on(
                "v",
                at,
                end.min(100),
                looks[rnd.below(3) as usize].clone(),
            ));
            at = end + rnd.below(4) as Us;
        }
        old.push(on("w", 0, 100, big()));
        let ranges: Vec<(Us, Us)> = (0..1 + rnd.below(8))
            .map(|_| {
                let start = rnd.below(100) as Us;
                (start, start + 1 + rnd.below(15) as Us)
            })
            .collect();
        let look = looks[rnd.below(4) as usize].clone();
        let p = project(old.clone());
        let next = klipolo_model::apply_ops(&p, &[set("v", &ranges, look.clone())])
            .unwrap_or_else(|e| panic!("case {case}: {e:?}"));
        let (was, now) = (LookIndex::new(&old), LookIndex::new(&next.word_looks));
        for t in 0..120 {
            let inside = ranges.iter().any(|&(a, b)| a <= t && t < b);
            let want = if inside {
                (!look.is_empty()).then_some(&look)
            } else {
                was.at("v", t)
            };
            assert_eq!(now.at("v", t), want, "case {case} at {t}: {ranges:?}");
            assert_eq!(now.at("w", t), was.at("w", t), "case {case}: w at {t}");
        }
    }
}

/// Real scale (release, `--ignored`; its time goes in the task report): "hide the ums" on a two-hour take, 5,000 words
/// over a list at the cap's 15,000 looks, as one op.
#[test]
#[ignore = "real scale: run in release with --ignored"]
fn five_thousand_words_on_fifteen_thousand_looks_is_one_pass() {
    let old: Vec<WordLook> = (0..15_000)
        .map(|i| on("v", i * 400_000, i * 400_000 + 200_000, big()))
        .collect();
    let p = project(old);
    let ranges: Vec<(Us, Us)> = (0..5_000)
        .map(|i| (i * 1_200_000 + 250_000, i * 1_200_000 + 350_000))
        .collect();
    let hide = Look {
        hidden: Some(true),
        ..Look::default()
    };
    let op = set("v", &ranges, hide);
    let started = std::time::Instant::now();
    let next = klipolo_model::apply_ops(&p, &[op]).unwrap();
    let took = started.elapsed();
    println!(
        "one op, 5,000 ranges over 15,000 looks: {took:?} ({} looks after)",
        next.word_looks.len()
    );
    assert_eq!(next.word_looks.len(), 20_000);
}
