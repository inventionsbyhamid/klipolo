use std::sync::Arc;

use klipolo_model::contrib::builtin;
use klipolo_model::ops::Op;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::time::{Us, secs_to_us};
use klipolo_model::timeline::{Clip, ClipSource, Transition};

use super::lookahead::{
    backward_plan, titles_behind, titles_only, upcoming_backward, upcoming_starts, upcoming_titles,
};
use crate::plan::{Layer, PlanContext, frame_plan};

fn s(seconds: f64) -> Us {
    secs_to_us(seconds)
}

/// `clips` as (id, asset, in s, out s) over 10 s assets "screen" and "broll", with a transition `effect` out of
/// `from` (`seconds` long, or the longest it can be when None).
fn timeline(
    clips: &[(&str, &str, f64, f64)],
    from: &str,
    effect: &str,
    seconds: Option<f64>,
) -> PlanContext {
    let mut p = Project::new("Lookahead");
    p.assets = ["screen", "broll"]
        .map(|id| Asset {
            id: id.into(),
            kind: AssetKind::Video,
            path: format!("/media/{id}"),
            name: id.into(),
            duration_us: s(10.0),
            width: 1920,
            height: 1080,
            fps: 30.0,
            has_audio: false,
            audio_lead_us: None,
            made_with: None,
            rotation: None,
        })
        .into();
    p.timeline.main = clips
        .iter()
        .map(|&(id, asset, from, to)| Clip {
            id: id.into(),
            ..Clip::new(ClipSource::Asset { id: asset.into() }, s(from), s(to))
        })
        .collect();
    let i = p.timeline.main.iter().position(|c| c.id == from).unwrap();
    let duration_us =
        seconds.map_or_else(|| klipolo_model::transitions::max_transition_us(&p, i), s);
    let transition = Op::SetTransition {
        clip_id: from.into(),
        transition: Some(Transition {
            effect: effect.into(),
            duration_us,
            params: Default::default(),
        }),
    };
    PlanContext::new(Arc::new(
        klipolo_model::apply_ops(&p, &[transition]).unwrap(),
    ))
}

fn starts(ctx: &PlanContext, after: f64, until: f64) -> Vec<Us> {
    let mut out = Vec::new();
    upcoming_starts(ctx, s(after), s(until), &mut out);
    out
}

/// Whether the plan at `at` is a transition whose outgoing clip starts with its window.
fn outgoing_starts(ctx: &PlanContext, at: Us) -> bool {
    match &frame_plan(ctx, at, FormatKind::Landscape).layers[0] {
        Layer::Transition(t) => t.outgoing_starts,
        other => panic!("expected a transition at {at}, got {other:?}"),
    }
}

#[test]
fn a_transition_is_warmed_from_its_window_not_its_cut() {
    // One recording cut in three: 0–4 s and 5–9 s joined by a 1 s dissolve (window 3.5–4.5 s, cut at 4 s), then a
    // plain cut at 8 s to 9.5–10 s.
    let clips = [
        ("a", "screen", 0.0, 4.0),
        ("b", "screen", 5.0, 9.0),
        ("c", "screen", 9.5, 10.0),
    ];
    let ctx = timeline(&clips, "a", builtin::TRANSITION_DISSOLVE, Some(1.0));
    assert_eq!(starts(&ctx, 3.0, 9.0), [s(3.5), s(8.0)]);
    // "a" is playing when the window opens: only the incoming side is warmed.
    assert!(!outgoing_starts(&ctx, s(3.5)));
}

#[test]
fn a_clip_that_opens_its_window_is_warmed_with_both_sides() {
    // "b" (broll, 0.4 s, after a plain cut at 4 s) dissolves into "c" for as long as it can: 0.8 s, whose first half
    // is all of "b", so the window opens at 4 s with "b"'s first frame. Nothing plays "b" before it.
    let clips = [
        ("a", "screen", 0.0, 4.0),
        ("b", "broll", 2.0, 2.4),
        ("c", "screen", 6.0, 9.0),
    ];
    let ctx = timeline(&clips, "b", builtin::TRANSITION_DISSOLVE, None);
    assert_eq!(ctx.transitions[0].start_us, s(4.0));
    assert_eq!(starts(&ctx, 3.5, 5.0), [s(4.0)]);
    assert!(outgoing_starts(&ctx, s(4.0)));
}

#[test]
fn an_uninstalled_transition_is_warmed_as_a_cut() {
    // The first test's timeline with a transition this build doesn't have: its frames are a cut, so the cut at 4 s is
    // warmed (the incoming clip shows from there) and the window's start isn't.
    let clips = [
        ("a", "screen", 0.0, 4.0),
        ("b", "screen", 5.0, 9.0),
        ("c", "screen", 9.5, 10.0),
    ];
    let ctx = timeline(&clips, "a", "com.acme.fx/transition.spin", Some(1.0));
    assert_eq!(starts(&ctx, 3.0, 9.0), [s(4.0), s(8.0)]);
}

fn behind(ctx: &PlanContext, from: f64, before: f64) -> Vec<Us> {
    let mut out = Vec::new();
    upcoming_backward(ctx, s(from), s(before), &mut out);
    out
}

/// The assets whose decoders warming `plan` opens.
fn warmed_assets(plan: &crate::plan::FramePlan) -> Vec<String> {
    plan.video_sources()
        .into_iter()
        .map(|(_, src)| src.asset_id.to_string())
        .collect()
}

#[test]
fn backward_warms_the_frame_before_a_cut_and_a_transition_from_its_end() {
    // "a" (screen 0–4 s) dissolves into "b" (broll 5–9 s) across 3.5–4.5 s; "c" follows "b" at a plain cut at 8 s.
    let clips = [
        ("a", "screen", 0.0, 4.0),
        ("b", "broll", 5.0, 9.0),
        ("c", "screen", 9.5, 10.0),
    ];
    let ctx = timeline(&clips, "a", builtin::TRANSITION_DISSOLVE, Some(1.0));
    // Entered from their ends: "b"'s last frame before the cut at 8 s, and the window's last instant (the cut at 4 s
    // inside it is left to the window).
    assert_eq!(behind(&ctx, 3.0, 9.0), [s(4.5) - 1, s(8.0) - 1]);
    // Each once: the next lookback starts where this one stopped.
    assert_eq!(behind(&ctx, 3.0, 4.0), Vec::<Us>::new());
    // Entering the window from its end warms only its outgoing clip: "b" is the one already playing.
    let plan = backward_plan(&ctx, s(4.5) - 1, FormatKind::Landscape);
    assert_eq!(warmed_assets(&plan), ["screen"]);
}

#[test]
fn backward_warms_both_sides_of_a_window_its_incoming_clip_ends_with() {
    // "b" (broll, 1 s) takes all of its length after the cut into its incoming dissolve: the window ends with "b", so
    // going backward nothing has played "b" before it either.
    let clips = [
        ("a", "screen", 0.0, 4.0),
        ("b", "broll", 2.0, 3.0),
        ("c", "screen", 6.0, 9.0),
    ];
    let ctx = timeline(&clips, "a", builtin::TRANSITION_DISSOLVE, Some(2.0));
    let w = ctx.transitions[0];
    assert_eq!((w.start_us, w.end_us), (s(3.0), s(5.0)));
    assert_eq!(behind(&ctx, 4.0, 6.0), [s(5.0) - 1]);
    let plan = backward_plan(&ctx, s(5.0) - 1, FormatKind::Landscape);
    assert_eq!(warmed_assets(&plan), ["screen", "broll"]);
}

/// A motion graphic coming up is warmed, so its instance is built before its first frame; a shader
/// generator isn't (it has nothing to build ahead). With the titles' plan, its scenes
/// and titles alone, so warming it opens no decoder.
#[test]
fn a_scene_layer_coming_up_is_warmed() {
    use klipolo_model::layout::{Anchor, BoxN, Layout};
    use klipolo_model::timeline::{Item, ItemContent, Track, TrackKind};
    let layer = |id: &str, source: &str, start: f64| Item {
        id: id.into(),
        start_us: s(start),
        duration_us: s(0.5),
        attach: None,
        layout: Layout::new(
            Anchor::TopLeft,
            BoxN {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        ),
        content: ItemContent::Generator {
            source: source.into(),
            params: Default::default(),
            still: false,
            offset_us: 0,
            span_us: None,
            seed_from: None,
        },
        effects: Vec::new(),
    };
    let mut p = Project::new("Scenes ahead");
    let mut track = Track::new(TrackKind::Overlay, "titles");
    track.items = vec![
        layer("title", "klipolo.test/scene.title", 2.0),
        layer("wash", builtin::GENERATOR_GRADIENT, 3.0),
    ];
    p.timeline.tracks.push(track);
    let ctx = PlanContext::new(Arc::new(p));
    assert!(starts(&ctx, 1.0, 4.0).is_empty(), "not with the whole plan");
    let mut ahead = Vec::new();
    upcoming_titles(&ctx, s(1.0), s(4.0), &mut ahead);
    assert_eq!(ahead, [s(2.0)]);
}

/// A title coming up is warmed a second ahead, and going backward from its last instant,
/// each with a plan of its text alone: warming it builds it ahead and opens no decoder (the main clip's plan there,
/// a second ahead of the playhead, would move the spare lane to it).
#[test]
fn a_title_coming_up_is_warmed_ahead_with_no_decoder() {
    use klipolo_model::layout::{Anchor, BoxN, Layout};
    use klipolo_model::timeline::{Item, ItemContent, TextStyle, Track, TrackKind};

    // The first test's timeline (a dissolve across 3.5–4.5 s), with a title from 2 s to 3 s.
    let clips = [("a", "screen", 0.0, 4.0), ("b", "screen", 5.0, 9.0)];
    let ctx = timeline(&clips, "a", builtin::TRANSITION_DISSOLVE, Some(1.0));
    let mut p = (*ctx.project).clone();
    let mut titles = Track::new(TrackKind::Overlay, "Titles");
    titles.items = vec![Item {
        id: "title".into(),
        start_us: s(2.0),
        duration_us: s(1.0),
        attach: None,
        layout: Layout::new(
            Anchor::TopLeft,
            BoxN {
                x: 0.1,
                y: 0.1,
                w: 0.8,
                h: 0.2,
            },
        ),
        content: ItemContent::Text {
            text: "Hello".into(),
            style: TextStyle::default(),
        },
        effects: Vec::new(),
    }];
    p.timeline.tracks = vec![titles];
    let ctx = PlanContext::new(Arc::new(p));
    let mut out = Vec::new();
    upcoming_titles(&ctx, s(1.0), s(2.5), &mut out);
    assert_eq!(out, [s(2.0)], "its start, a second ahead");
    assert_eq!(starts(&ctx, 1.0, 2.5), Vec::<Us>::new(), "no decoder start");
    titles_behind(&ctx, s(2.5), s(3.5), &mut out);
    assert_eq!(out, [s(3.0) - 1], "going backward, its last instant");
    let plan = titles_only(frame_plan(&ctx, s(2.0), FormatKind::Landscape));
    assert!(
        matches!(plan.layers.as_slice(), [Layer::Text(t)] if &*t.text == "Hello"),
        "the title alone: {:?}",
        plan.layers
    );
}
