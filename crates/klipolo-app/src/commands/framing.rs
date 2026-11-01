//! How a main-track clip is framed in each format, as the engine draws it: its own framing, else the model's default
//! (`klipolo_model::framing::effective_framing`, the rule the engine and chat's tools read), and whether it can follow
//! the cursor at all (`framing::can_follow`), with which part of the footage Fill shows from the centre in each format
//! (`klipolo_engine::plan::fill_crop`), and whether it can follow the speaker (`subject::clips::footage`: an imported
//! video or a camera take). The clip card's Fit, Fill and Fill focus show this answer, never a copy of these rules.
//! Read-only: it changes nothing.

use std::collections::BTreeMap;

use klipolo_analysis::subject::clips::footage;
use klipolo_engine::plan::fill_crop;
use klipolo_model::coded::Said;
use klipolo_model::framing::{can_follow, effective_framing};
use klipolo_model::layout::BoxN;
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::Framing;
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::project::{ProjectSessionState, with_open};

/// A clip's framing as the engine draws it (`clip_framing`).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClipFraming {
    /// Every format's framing: the clip's own, else the default.
    pub framing: BTreeMap<FormatKind, Framing>,
    /// It can follow the cursor in a vertical or square version: a screen take with a cursor track.
    pub follows: bool,
    /// It can follow the speaker where Fill crops it: an imported video, or a camera take.
    pub speaker: bool,
    /// Every format's Fill crop from the centre, normalised to what the clip's crop leaves of its footage: narrower
    /// than that where Fill crops across, shorter where it crops down, all of it where nothing is cropped; empty for a
    /// clip with no video.
    pub fill_crop: BTreeMap<FormatKind, BoxN>,
}

/// The open project's clip `clip_id`'s framing in every format.
#[tauri::command(async)]
pub fn clip_framing(
    state: State<'_, ProjectSessionState>,
    clip_id: String,
) -> Result<ClipFraming, Said> {
    framing_of(&state, &clip_id)
}

/// [`clip_framing`]: `model.notFound.clip` for a clip that's gone (removed since the page asked), `project.none` with
/// no project open. O(clips).
pub(crate) fn framing_of(state: &ProjectSessionState, clip_id: &str) -> Result<ClipFraming, Said> {
    with_open(state, |open| {
        let p = open.document.project();
        let clip = (p.timeline.main.iter())
            .find(|c| c.id == clip_id)
            .ok_or_else(|| Said::code("model.notFound.clip"))?;
        let formats = [
            FormatKind::Landscape,
            FormatKind::Portrait,
            FormatKind::Square,
        ];
        Ok(ClipFraming {
            framing: formats
                .into_iter()
                .map(|f| (f, effective_framing(p, clip, f)))
                .collect(),
            follows: can_follow(p, clip),
            speaker: footage(p, clip).is_some(),
            fill_crop: formats
                .into_iter()
                .filter_map(|f| Some((f, fill_crop(p, clip, f)?)))
                .collect(),
        })
    })
}

#[cfg(test)]
mod tests {
    use klipolo_model::cursor_style::RecordingCursor;
    use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
    use klipolo_model::project::{Asset, AssetKind, Project};
    use klipolo_model::recording::{
        Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
    };
    use klipolo_model::timeline::{Clip, ClipSource, FramingMode};

    use super::*;
    use crate::project::create_in;

    fn video(id: &str) -> Asset {
        Asset {
            id: id.into(),
            kind: AssetKind::Video,
            path: format!("/media/{id}.mp4"),
            name: format!("{id}.mp4"),
            duration_us: 60_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
        }
    }

    fn clip(id: &str, source: ClipSource) -> Clip {
        let mut clip = Clip::new(source, 0, 10_000_000);
        clip.id = id.into();
        clip
    }

    /// An imported video (c1), then two clips of a screen recording with a cursor (c2, c3); c3 has a portrait framing
    /// of its own, a Fill near the top that doesn't follow.
    fn project(own: Framing) -> Project {
        let mut p = Project::new("Framing");
        p.assets = vec![video("broll"), video("screen")];
        p.recordings = vec![Recording {
            id: "take".into(),
            kind: RecordingKind::Screen,
            sources: vec![RecordingSource {
                id: "screen".into(),
                role: SourceRole::Screen,
                label: "capture.source.screen".into(),
                runs: vec![SourceRun {
                    asset_id: "screen".into(),
                    offset_us: 0,
                    duration_us: 60_000_000,
                    drift_ppm: 0.0,
                    file_in_us: 0,
                }],
                nudge_us: 0,
                mix: SourceMix::default(),
                synced: None,
            }],
            primary: "screen".into(),
            cursor: Some(RecordingCursor::burned_in("/media/take/cursor.jsonl")),
            markers: Vec::new(),
            game: None,
        }];
        let take = || ClipSource::Recording { id: "take".into() };
        let mut c3 = clip("c3", take());
        c3.framing.insert(Portrait, own);
        p.timeline.main = vec![
            clip("c1", ClipSource::Asset { id: "broll".into() }),
            clip("c2", take()),
            c3,
        ];
        p
    }

    #[test]
    fn a_clips_framing_is_what_the_engine_draws_in_every_format() {
        let dir = tempfile::tempdir().unwrap();
        let state = ProjectSessionState::default();
        assert_eq!(
            framing_of(&state, "c1").unwrap_err(),
            Said::code("project.none")
        );
        let own = Framing {
            mode: FramingMode::Fill,
            focus_x: 0.5,
            focus_y: 0.2,
            follow: None,
        };
        create_in(&state, dir.path(), project(own)).unwrap();

        // Whether a clip can follow is the model's too: a screen take with a cursor track can, an imported video
        // can't.
        assert!(framing_of(&state, "c2").unwrap().follows);
        assert!(!framing_of(&state, "c1").unwrap().follows);
        // An imported video can follow its speaker; a screen take follows its cursor instead.
        assert!(framing_of(&state, "c1").unwrap().speaker);
        assert!(!framing_of(&state, "c2").unwrap().speaker);
        let c2 = framing_of(&state, "c2").unwrap().framing;
        assert_eq!(
            c2.keys().copied().collect::<Vec<_>>(),
            [Landscape, Portrait, Square]
        );
        assert_eq!(
            (c2[&Landscape].mode, c2[&Landscape].follow),
            (FramingMode::Fit, None)
        );
        // Portrait and square follow the cursor by default: Fill with follow.
        assert_eq!(c2[&Portrait].mode, FramingMode::Fill);
        assert!(c2[&Portrait].follow.is_some() && c2[&Square].follow.is_some());
        assert_eq!(framing_of(&state, "c3").unwrap().framing[&Portrait], own);
        // An imported video fills from the centre in every format and never follows.
        let c1 = framing_of(&state, "c1").unwrap().framing;
        for format in [Landscape, Portrait, Square] {
            assert_eq!(
                (
                    c1[&format].mode,
                    c1[&format].focus_x,
                    c1[&format].focus_y,
                    c1[&format].follow
                ),
                (FramingMode::Fill, 0.5, 0.5, None)
            );
        }
        // Fill crops a 16:9 video across in 9:16 and 1:1 and not at all in 16:9; the screen take is 16:9 too.
        let crop = framing_of(&state, "c1").unwrap().fill_crop;
        assert_eq!(crop[&Landscape], BoxN::FULL);
        assert!(crop[&Portrait].w < 0.32 && crop[&Portrait].h == 1.0);
        assert!((crop[&Square].w - 0.5625).abs() < 1e-9 && crop[&Square].h == 1.0);
        assert_eq!(framing_of(&state, "c2").unwrap().fill_crop, crop);
        assert_eq!(
            framing_of(&state, "gone").unwrap_err(),
            Said::code("model.notFound.clip")
        );
    }
}
