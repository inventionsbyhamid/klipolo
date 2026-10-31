//! What chat reads from a clip's analysis, as the chat crate's own neutral types: the scene log's items
//! (`klipolo_chat::SceneEntry`) and the speaker turns (`klipolo_chat::SpeakerTurn`). The conversion lives here, in the
//! app, so the chat crate never sees the analysis crate or the engine.

use klipolo_analysis::providers::SceneKind;
use klipolo_analysis::scenes::SceneLog;
use klipolo_analysis::transcript::Turn;

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
    turns
        .iter()
        .map(|t| klipolo_chat::SpeakerTurn {
            start_us: t.start_us,
            end_us: t.end_us,
            speaker: t.speaker.clone(),
        })
        .collect()
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
