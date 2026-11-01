//! What chat reads from a clip's analysis, as the chat crate's own neutral types: the scene log's items
//! (`klipolo_chat::SceneEntry`) and the speaker turns (`klipolo_chat::SpeakerTurn`). The conversion lives here, in the
//! app, so the chat crate never sees the analysis crate or the engine.

use std::collections::HashMap;

use klipolo_analysis::providers::SceneKind;
use klipolo_analysis::scenes::SceneLog;
use klipolo_analysis::subject::clips::footage;
use klipolo_analysis::transcript::Turn;
use klipolo_model::Project;
use klipolo_model::fill_focus::{FillFocus, Voice};

use super::{AnalysisState, AnalysisStep, AssetState};

impl AnalysisState {
    /// How far `asset_id`'s analysis is, as chat's compiler asks: the scene log running is still watching (its words
    /// are in by then: the scene log comes last), any other step still listening; a transcript (cached, or kept when a
    /// later step failed) is done; nothing yet is not started.
    pub fn chat_state(&self, asset_id: &str) -> klipolo_chat::AnalysisState {
        match self.state_of(asset_id) {
            AssetState::Running {
                step: AnalysisStep::Scenes,
                ..
            } => klipolo_chat::AnalysisState::Watching,
            AssetState::Running { .. } => klipolo_chat::AnalysisState::Transcribing,
            _ if self
                .analysis_for(asset_id)
                .is_some_and(|a| a.transcript.is_some()) =>
            {
                klipolo_chat::AnalysisState::Done
            }
            _ => klipolo_chat::AnalysisState::NotStarted,
        }
    }
}

/// A scene log as chat reads it: each item in the clip's file time, its kind in one word.
pub fn scene_entries(log: &SceneLog) -> klipolo_chat::SceneLog {
    klipolo_chat::SceneLog {
        items: log
            .items
            .iter()
            .map(|item| klipolo_chat::SceneEntry {
                start_us: item.start_us,
                end_us: item.end_us,
                kind: kind_word(item.kind).to_owned(),
                who: item.who.clone(),
                text: item.text.clone(),
                on_screen_text: (!item.on_screen_text.is_empty())
                    .then(|| item.on_screen_text.clone()),
            })
            .collect(),
    }
}

/// Speaker turns as chat reads them, in the clip's sound time.
pub fn speaker_turns(turns: &[Turn]) -> Vec<klipolo_chat::SpeakerTurn> {
    person_turns(turns, &HashMap::new())
}

/// [`speaker_turns`], each voice matched to a person (`people_voices`) named by that person's number ("P1"), as the
/// context's People line and `followSubject`'s focus name them; the rest by their label.
pub fn person_turns(
    turns: &[Turn],
    people: &HashMap<Voice, String>,
) -> Vec<klipolo_chat::SpeakerTurn> {
    turns
        .iter()
        .map(|t| klipolo_chat::SpeakerTurn {
            start_us: t.start_us,
            end_us: t.end_us,
            speaker: t.speaker.as_ref().map(|label| {
                let voice = Voice {
                    part: t.chunk,
                    label: label.clone(),
                };
                people.get(&voice).cloned().unwrap_or_else(|| label.clone())
            }),
        })
        .collect()
}

/// The person each voice of `asset_id`'s transcript was matched to, as "P1": from the people its clips' crops follow
/// (the primary format's first). Empty when no clip of it follows two or more people.
pub fn people_voices(p: &Project, asset_id: &str) -> HashMap<Voice, String> {
    let mut out = HashMap::new();
    let own =
        (p.timeline.main.iter()).filter(|c| footage(p, c).is_some_and(|f| f.asset_id == asset_id));
    for clip in own {
        let primary = clip.fill_focus.get(&p.settings.primary_format).into_iter();
        for focus in primary.chain(clip.fill_focus.values()) {
            let FillFocus::Speaker { people, .. } = focus else {
                continue;
            };
            for (i, person) in people.iter().enumerate() {
                for voice in &person.voices {
                    out.entry(voice.clone())
                        .or_insert_with(|| format!("P{}", i + 1));
                }
            }
        }
    }
    out
}

/// A scene kind as one word, as the model reads it.
fn kind_word(kind: SceneKind) -> &'static str {
    match kind {
        SceneKind::Shot => "shot",
        SceneKind::Speech => "speech",
        SceneKind::Action => "action",
        SceneKind::Reaction => "reaction",
        SceneKind::Text => "text",
        SceneKind::Topic => "topic",
        SceneKind::Other => "other",
    }
}
