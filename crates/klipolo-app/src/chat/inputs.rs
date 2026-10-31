//! A message's context and compiler inputs, built once from the project snapshot it is sent on and what each of that
//! project's clips' analysis has found then (`Findings`): the model reads the context, and its calls compile against
//! the same snapshot and inputs.

use std::collections::HashMap;

use klipolo_chat::{
    AnalysisState as ChatAnalysis, ChatContext, Inputs, SceneLog, SpeakerTurn, View, build_context,
};
use klipolo_model::Project;
use klipolo_model::project::AssetKind;
use klipolo_model::speech::Speech;

use super::sources::Findings;
use crate::analysis::chat::{scene_entries, speaker_turns};

/// The context the model reads for `project`, and the inputs its calls compile against: `message` (the times it says
/// are evidence) and `view` (the playhead, the selection) as sent.
pub(crate) fn gather(
    project: &Project,
    findings: &dyn Findings,
    message: &str,
    view: View,
) -> (ChatContext, Inputs) {
    let mut speech = Speech::default();
    let mut turns: HashMap<String, Vec<SpeakerTurn>> = HashMap::new();
    let mut scenes: HashMap<String, SceneLog> = HashMap::new();
    let mut analysis: HashMap<String, ChatAnalysis> = HashMap::new();
    let levels = findings.levels();
    // Pictures have nothing to analyse; every video and sound file is asked once.
    let files = (project.assets.iter()).filter(|a| a.kind != AssetKind::Image);
    for asset in files {
        let id = &asset.id;
        analysis.insert(id.clone(), findings.state(id));
        if let Some(level) = levels.get(id) {
            speech.levels.insert(id.clone(), level.clone());
        }
        let Some(found) = findings.found(id) else {
            continue;
        };
        if let Some(transcript) = &found.transcript {
            speech.transcripts.insert(id.clone(), transcript.clone());
        }
        if !found.turns.is_empty() {
            turns.insert(id.clone(), speaker_turns(&found.turns));
        }
        if let Some(log) = &found.scenes {
            scenes.insert(id.clone(), scene_entries(log));
        }
    }
    let context = build_context(project, &speech, &turns, &scenes, &view);
    let inputs = Inputs {
        message: message.to_owned(),
        view,
        refs: context.refs.clone(),
        speech,
        scenes,
        analysis,
    };
    (context, inputs)
}
