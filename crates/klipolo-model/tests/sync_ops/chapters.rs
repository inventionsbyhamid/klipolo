//! A chapter's clip may reach a little past its run into the next chapter on a recording, and no further:
//! `useRecording` takes it when the next run starts where its run ends, up to 0.5 s past it.

use klipolo_model::apply::apply_ops;
use klipolo_model::ops::Op;
use klipolo_model::project::{AssetKind, SourceRun};
use klipolo_model::timeline::{Clip, ClipSource};

use super::common::{self, secs};
use super::{file, run, synced};

/// Moves a clip of `cam1` from 500 s to `to_s` onto a recording whose picture runs are `cam1` (a file `first_s` long)
/// for 599.933 s, then `cam2` from `second_at_s` on the clock.
fn use_chapter(first_s: f64, second_at_s: f64, to_s: f64) -> Result<(), String> {
    let mut p = common::imported();
    p.assets.extend([
        file("cam1", AssetKind::Video, first_s),
        file("cam2", AssetKind::Video, 300.0),
    ]);
    let mut c = Clip::new(
        ClipSource::Asset { id: "cam1".into() },
        secs(500.0),
        secs(to_s),
    );
    c.id = "c7".into();
    p.timeline.main.push(c);
    let mut r = synced();
    r.sources[0].runs = vec![
        run("cam1", 0, secs(599.933), 0.0),
        SourceRun {
            offset_us: secs(second_at_s),
            ..run("cam2", 0, secs(300.0), 0.0)
        },
    ];
    let ops = [
        Op::AddRecording { recording: r },
        Op::UseRecording {
            clip_id: "c7".into(),
            recording_id: "r9".into(),
        },
    ];
    apply_ops(&p, &ops)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn a_chapter_clip_reaches_a_little_into_the_next_and_no_further() {
    // 67 ms past its run, the next chapter starting there: taken.
    assert_eq!(use_chapter(600.0, 599.933, 600.0), Ok(()));
    // 0.667 s past it: refused, as is a clip past a run whose next one starts a second later (no chapter join).
    for (first_s, second_at_s, to_s) in [(600.6, 599.933, 600.6), (600.0, 601.0, 600.0)] {
        let refused = use_chapter(first_s, second_at_s, to_s).unwrap_err();
        assert!(refused.contains("notThisFile"), "{refused}");
    }
}
