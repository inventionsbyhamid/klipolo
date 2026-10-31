//! Which edits can change a revision's caption pages: a committed revision's feed compares what pages are made
//! from; a preview's dry run asks of its op. Neither builds pages to find out.

use klipolo_model::Op;
use klipolo_model::ops::SourcePatch;
use klipolo_model::project::Project;
use klipolo_model::speech::run_spans;

/// Whether `after`'s pages can differ from `before`'s for the same speech: a committed revision's feed carries no ops,
/// so it compares what pages are made from. Captions turned on or off, another style, look, size or case, other
/// fixes or looks (shared lists: a pointer compare while untouched), a different speech map (clips, speeds, source
/// nudges and mixes, through `run_spans`: O(clips)), or a transition added or removed (it makes a split a cut, which
/// which ends a page; O(clips)). Colours, fonts and places don't page. The app's caption pages for the
/// lane and the canvas keep their build by it too.
pub fn pages_may_differ(before: &Project, after: &Project) -> bool {
    let (a, b) = (&before.captions, &after.captions);
    if !a.enabled && !b.enabled {
        return false;
    }
    a.enabled != b.enabled
        || a.style != b.style
        || a.variant != b.variant
        || before.settings.formats != after.settings.formats
        || before.word_fixes != after.word_fixes
        || before.word_looks != after.word_looks
        || run_spans(before) != run_spans(after)
        || !before
            .timeline
            .main
            .iter()
            .map(|c| c.transition_out.is_some())
            .eq(after
                .timeline
                .main
                .iter()
                .map(|c| c.transition_out.is_some()))
}

/// Whether a dry run's `op` (on the committed project `p`) can move a word or a page, or change which pages there are:
/// the preview's dry runs then make their previewed format's pages (a few ms for two hours, at most once per animation
/// frame); otherwise they keep the committed ones. An op not listed here counts as moving them.
pub(crate) fn may_move_captions(p: &Project, op: &Op) -> bool {
    match op {
        // Turning captions on or off changes the pages, and so may a style or look, whose rules differ; a colour, a
        // font or a place doesn't.
        Op::SetCaptions { captions } => {
            captions.enabled != p.captions.enabled
                || captions.style != p.captions.style
                || captions.variant != p.captions.variant
        }
        Op::UpdateClip { patch, .. } => patch.speed.is_some(),
        // A transition makes a split a cut (a page ends there), and removing one joins the shot again.
        Op::SetTransition { .. } => true,
        // A source's name, or how it was lined up, moves no word; its nudge or re-timed runs shift its words, and
        // its mix stays counted.
        Op::UpdateRecordingSource { patch, .. } => {
            let SourcePatch {
                label: _,
                mix,
                nudge_us,
                runs,
                synced: _,
            } = patch;
            mix.is_some() || nudge_us.is_some() || runs.is_some()
        }
        Op::Rename { .. }
        | Op::SetBackground { .. }
        | Op::SetAudio { .. }
        | Op::AddAssets { .. }
        | Op::AddRecording { .. }
        | Op::AddLut { .. }
        | Op::RemoveLut { .. }
        | Op::AddTrack { .. }
        | Op::RemoveTrack { .. }
        | Op::MoveTrack { .. }
        | Op::UpdateTrack { .. }
        | Op::AddItem { .. }
        | Op::UpdateItem { .. }
        | Op::MoveItem { .. }
        | Op::TrimItem { .. }
        | Op::SplitItem { .. }
        | Op::RemoveItem { .. }
        | Op::SetAttach { .. }
        | Op::AddEffect { .. }
        | Op::UpdateEffect { .. }
        | Op::RemoveEffect { .. }
        | Op::MoveEffect { .. }
        | Op::AddMarker { .. }
        | Op::RemoveMarker { .. } => false,
        // A clip's own mix of a picture source decides whether the camera's words play over its gaps
        // `speech::fill`), so its dry runs (a volume dragged to 0, a mute previewed) make their own cues
        // m3); a mic's own mix moves no word. O(recordings × sources).
        Op::SetClipSourceMix { source_id, .. } => p
            .recordings
            .iter()
            .flat_map(|r| &r.sources)
            .any(|s| s.id == *source_id && s.role.is_video()),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use klipolo_model::Op;
    use klipolo_model::contrib::ContribId;
    use klipolo_model::ops::SourcePatch;
    use klipolo_model::project::{
        CaptionSettings, Project, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole,
    };

    use super::{may_move_captions, pages_may_differ};

    /// A style or look changes the lines (a hover over a style tile previews its own); a colour keeps them.
    #[test]
    fn a_style_or_look_moves_the_pages_and_a_colour_doesnt() {
        let mut p = Project::new("Cues");
        p.captions.enabled = true;
        p.captions.style = Some(ContribId::builtin("caption.boxed"));
        let set = |captions: CaptionSettings| Op::SetCaptions { captions };
        let restyled = set(CaptionSettings {
            style: Some(ContribId::builtin("caption.clean")),
            ..p.captions.clone()
        });
        let darker = set(CaptionSettings {
            variant: Some("dark".into()),
            ..p.captions.clone()
        });
        let recoloured = set(CaptionSettings {
            accent: Some("#EA403F".into()),
            ..p.captions.clone()
        });
        assert!(may_move_captions(&p, &restyled));
        assert!(may_move_captions(&p, &darker));
        assert!(!may_move_captions(&p, &recoloured));
    }

    /// A committed revision keeps the previous pages unless what they're made from changed: a rename or a colour
    /// keeps them; a word's look, a split (two spans) or captions turned on make them again.
    #[test]
    fn a_revision_keeps_its_pages_unless_what_they_are_made_of_changed() {
        use klipolo_model::captions::looks::{Look, WordLook};
        use klipolo_model::project::{Asset, AssetKind};
        use klipolo_model::timeline::{Clip, ClipSource};

        let mut p = Project::new("Pages");
        p.captions.enabled = true;
        p.assets = vec![Asset {
            id: "v".into(),
            kind: AssetKind::Video,
            path: "/media/v.mp4".into(),
            name: "v".into(),
            duration_us: 10_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
        }];
        p.timeline.main = vec![Clip::new(
            ClipSource::Asset { id: "v".into() },
            0,
            10_000_000,
        )];
        let mut renamed = p.clone();
        renamed.name = "Renamed".into();
        renamed.captions.accent = Some("#EA403F".into());
        assert!(!pages_may_differ(&p, &renamed));
        let mut looked = p.clone();
        looked.word_looks = vec![WordLook {
            asset_id: "v".into(),
            start_us: 0,
            end_us: 1,
            look: Look {
                hidden: Some(true),
                ..Look::default()
            },
        }]
        .into();
        assert!(pages_may_differ(&p, &looked));
        let mut split = p.clone();
        split.timeline.main[0].source_out_us = 4_000_000;
        assert!(pages_may_differ(&p, &split));
        let mut off = p.clone();
        off.captions.enabled = false;
        assert!(pages_may_differ(&off, &p));
        let mut off_renamed = off.clone();
        off_renamed.name = "Off".into();
        off_renamed.timeline.main.clear();
        assert!(
            !pages_may_differ(&off, &off_renamed),
            "off both times: no pages either way"
        );
    }

    /// A transition at a split left in place makes it a cut, so the words either side go on two
    /// pages. Adding or removing one pages again, in a dry run and in a committed revision, so the preview shows what
    /// an export (which pages afresh) draws.
    #[test]
    fn a_transition_at_a_split_left_in_place_pages_again() {
        use std::collections::HashMap;
        use std::sync::Arc;

        use klipolo_model::captions::captions;
        use klipolo_model::project::{Asset, AssetKind, FormatKind};
        use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
        use klipolo_model::timeline::{Clip, ClipSource, Transition};

        let mut p = Project::new("Pages");
        p.captions.enabled = true;
        p.captions.style = Some(ContribId::builtin("caption.clean"));
        p.assets = vec![Asset {
            id: "v".into(),
            kind: AssetKind::Video,
            path: "/media/v.mp4".into(),
            name: "v".into(),
            duration_us: 10_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
        }];
        let mut c1 = Clip::new(ClipSource::Asset { id: "v".into() }, 0, 2_000_000);
        c1.id = "c1".into();
        let mut c2 = Clip::new(ClipSource::Asset { id: "v".into() }, 2_000_000, 10_000_000);
        c2.id = "c2".into();
        p.timeline.main = vec![c1, c2];
        let word = |text: &str, start: i64| Word {
            text: text.into(),
            start_us: start,
            end_us: start + 200_000,
            confidence: 0.9,
            kind: WordKind::Word,
        };
        let t = Transcript {
            key: "k".into(),
            language: "en".into(),
            model: "m".into(),
            covered_us: 10_000_000,
            complete: true,
            words: vec![
                word("We", 1_500_000),
                word("go", 1_750_000),
                word("on", 2_050_000),
            ],
        };
        let speech = Speech {
            transcripts: HashMap::from([("v".to_owned(), Arc::new(t))]),
            ..Speech::default()
        };
        let dissolve = Transition {
            effect: ContribId::builtin("transition.dissolve"),
            duration_us: 200_000,
            params: Default::default(),
        };
        let mut joined = p.clone();
        joined.timeline.main[0].transition_out = Some(dissolve.clone());
        let pages = |p: &Project| {
            captions(p, &speech).pages[&FormatKind::Landscape]
                .pages
                .len()
        };
        assert_eq!(
            (pages(&p), pages(&joined)),
            (1, 2),
            "the transition makes the split a cut"
        );
        assert!(pages_may_differ(&p, &joined) && pages_may_differ(&joined, &p));
        let set = Op::SetTransition {
            clip_id: "c1".into(),
            transition: Some(dissolve),
        };
        assert!(may_move_captions(&p, &set));
    }

    #[test]
    fn markers_mixes_and_source_names_keep_the_committed_pages() {
        let p = Project::new("Cues");
        let source = |patch| Op::UpdateRecordingSource {
            recording_id: "rec".into(),
            source_id: "mic".into(),
            patch,
        };
        let renamed = source(SourcePatch {
            label: Some("Voice".into()),
            ..SourcePatch::default()
        });
        let nudged = source(SourcePatch {
            nudge_us: Some(40_000),
            ..SourcePatch::default()
        });
        let removed = Op::RemoveMarker {
            recording_id: "rec".into(),
            id: "m1".into(),
        };
        let mixed = Op::SetClipSourceMix {
            clip_id: "c1".into(),
            source_id: "mic".into(),
            mix: None,
        };
        for op in [&renamed, &removed, &mixed] {
            assert!(!may_move_captions(&p, op), "{op:?}");
        }
        assert!(may_move_captions(&p, &nudged));
    }

    #[test]
    fn a_clip_s_own_mix_of_the_camera_s_sound_makes_its_own_cues() {
        // The camera's words fill the mic's gaps where its clip mix is heard, so dragging that mix
        // (or previewing its mute) can change the cues; the mic's own mix can't.
        let mut p = Project::new("Cues");
        let source = |id: &str, role| RecordingSource {
            id: id.into(),
            role,
            label: String::new(),
            runs: Vec::new(),
            nudge_us: 0,
            mix: SourceMix::default(),
            synced: None,
        };
        p.recordings.push(Recording {
            id: "rec".into(),
            kind: RecordingKind::Camera,
            sources: vec![
                source("cam", SourceRole::Camera),
                source("mic", SourceRole::Mic),
            ],
            primary: "cam".into(),
            cursor: None,
            markers: Vec::new(),
            game: None,
        });
        let mix = |source: &str| Op::SetClipSourceMix {
            clip_id: "c1".into(),
            source_id: source.into(),
            mix: None,
        };
        assert!(may_move_captions(&p, &mix("cam")));
        assert!(!may_move_captions(&p, &mix("mic")));
    }
}
