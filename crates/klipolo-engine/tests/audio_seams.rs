//! Music laid back to back (`media::music`: a song shorter than the video plays again and again, in pieces of the same
//! file): where a piece plays the file to its end and the next starts it again, the mix plays straight through, as the
//! file loops cleanly. The plan marks those seams (`audio::plan::Seam`, `audio::plan::layers`) and the mixer leaves
//! edge ramps there.
mod support;
use std::sync::Arc;

use klipolo_engine::audio::mix::{Mixdown, RATE};
use klipolo_engine::audio::pcm::Pcm;
use klipolo_engine::audio::plan::{AudioSource, audio_plan};
use klipolo_engine::registry::Registry;
use klipolo_model::timeline::{Item, ItemContent, Track, TrackKind};
use support::*;

fn audio_track(items: Vec<Item>) -> Track {
    let mut t = Track::new(TrackKind::Audio, "Music");
    t.items = items;
    t
}

/// Music laid back to back under a longer video (a 1 s loop, three pieces): each seam, where a piece plays the file to
/// its end and the next starts it again, is marked in the plan and plays through at full level. The mixer's 3 ms edge
/// ramps used to dip the bed to silence there, about 6 ms at every seam. Its first start and last end keep theirs.
#[test]
fn music_laid_back_to_back_plays_through_its_seams() {
    let mut p = project();
    p.timeline.main.clear();
    p.assets
        .iter_mut()
        .find(|a| a.id == "music")
        .unwrap()
        .duration_us = secs(1.0);
    let piece = |id: &str, start: f64| Item {
        id: id.into(),
        start_us: secs(start),
        duration_us: secs(1.0),
        attach: None,
        layout: klipolo_model::layout::Layout::new(
            klipolo_model::layout::Anchor::TopLeft,
            bx(0.0, 0.0, 1.0, 1.0),
        ),
        content: ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: Some(-12.0),
        },
        effects: Vec::new(),
    };
    p.timeline.tracks = vec![audio_track(vec![
        piece("bed", 0.0),
        piece("bed-2", 1.0),
        piece("bed-3", 2.0),
    ])];
    let plan = audio_plan(&p, &Registry::builtin(), Arc::default());
    // A constant file: every sample around both seams at full level, in blocks that cross them.
    let pcm = Arc::new(Pcm::from_channels(vec![0.5; RATE as usize], 1));
    let mut mixdown = Mixdown::new(Arc::new(plan));
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let mut block = vec![0f32; 2 * 1_000];
    let mut lowest = f32::MAX;
    for k in 0..150u64 {
        mixdown.render(&lookup, k * 1_000, &mut block);
        if (40..56).contains(&k) || (88..104).contains(&k) {
            lowest = block.iter().copied().fold(lowest, f32::min);
        }
    }
    assert!(lowest > 0.49, "dips to {lowest} at a seam");
}
