//! A short's hook holds the 9:16 captions while it shows (`klipolo_model::captions::hook_holds_captions`): the frame's
//! caption state has no captions there in 9:16, has them in the other formats, and has them in 9:16 again as the hook
//! ends. Pure: no GPU.

use std::sync::Arc;

use klipolo_engine::plan::PlanContext;
use klipolo_engine::plan::captions::caption_layer;
use klipolo_engine::test_support::captions_project;
use klipolo_model::captions::{HOOK_BAND, captions};
use klipolo_model::layout::{Anchor, Layout};
use klipolo_model::project::FormatKind;
use klipolo_model::time::secs_to_us as s;
use klipolo_model::timeline::{Item, ItemContent, TextStyle, Track, TrackKind};

/// A word every half second from 0.2 s, word pop's captions on, and words in the hook's band from 0.2 s for `hook` s;
/// the main format `primary`.
fn context(hook: f64, primary: FormatKind) -> PlanContext {
    let words: Vec<(String, f64, f64)> = (0..20)
        .map(|k| (format!("w{k}"), 0.2 + 0.5 * k as f64, 0.5 + 0.5 * k as f64))
        .collect();
    let said: Vec<(&str, f64, f64)> = words.iter().map(|(w, a, b)| (w.as_str(), *a, *b)).collect();
    let (mut p, speech) = captions_project("caption.word-pop", &said);
    p.settings.primary_format = primary;
    let mut layout = Layout::new(Anchor::TopCenter, HOOK_BAND);
    if primary != FormatKind::Portrait {
        layout.rect.y = 0.1;
        layout.overrides.insert(FormatKind::Portrait, HOOK_BAND);
    }
    let mut track = Track::new(TrackKind::Overlay, "Words");
    track.items = vec![Item {
        id: "hook".into(),
        start_us: s(0.2),
        duration_us: s(hook),
        attach: None,
        layout,
        content: ItemContent::Text {
            text: "THE HOOK".into(),
            style: TextStyle::default(),
        },
        effects: Vec::new(),
    }];
    p.timeline.tracks.push(track);
    let pages = Arc::new(captions(&p, &speech));
    PlanContext::new(Arc::new(p)).with_captions(pages)
}

#[test]
fn the_9x16_captions_wait_while_the_hook_shows() {
    for primary in [FormatKind::Portrait, FormatKind::Landscape] {
        let ctx = context(2.5, primary);
        let at = |t: f64, f: FormatKind| caption_layer(&ctx, s(t), f, &mut Vec::new()).is_some();
        // While the hook shows (0.2 to 2.7 s): none in 9:16, the other formats as ever.
        for t in [0.3, 1.5, 2.6] {
            assert!(!at(t, FormatKind::Portrait), "{primary:?} at {t} s");
            assert!(at(t, FormatKind::Landscape), "{primary:?} at {t} s");
            assert!(at(t, FormatKind::Square), "{primary:?} at {t} s");
        }
        // As it ends, the 9:16 captions come back.
        assert!(at(2.75, FormatKind::Portrait), "{primary:?}");
    }
}

#[test]
fn words_elsewhere_hold_no_captions() {
    // The same words moved out of the band (say by hand): the captions show under them.
    let ctx = context(2.5, FormatKind::Portrait);
    let mut p = (*ctx.project).clone();
    p.timeline.tracks.last_mut().unwrap().items[0].layout.rect.y = 0.2;
    let moved = PlanContext::new(Arc::new(p)).with_captions(ctx.captions.clone());
    assert!(caption_layer(&moved, s(1.5), FormatKind::Portrait, &mut Vec::new()).is_some());
}
