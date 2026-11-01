//! A message's context and compiler inputs, built once from the project snapshot it is sent on and what each of that
//! project's clips' analysis has found then (`Findings`): the model reads the context, and its calls compile against
//! the same snapshot and inputs. The media the tools may name (`Library`) are what the library lists: the project's own
//! music, sounds and pictures (a sound file is music or a sound effect as the library's tabs say), then Klipolo's pack.
//! Another app's calls (MCP) compile against inputs built the same way (`inputs_for`), with no message.

use std::collections::{HashMap, HashSet};

use klipolo_chat::{
    AnalysisState as ChatAnalysis, ChatContext, Inputs, Library, LibraryItem, MediaKind, Refs,
    SceneLog, SpeakerTurn, View, build_context,
};
use klipolo_model::Project;
use klipolo_model::media::music::is_music;
use klipolo_model::project::AssetKind;
use klipolo_model::speech::Speech;
use klipolo_model::time::Us;
use klipolo_model::timeline::ItemContent;

use super::sources::Findings;
use crate::analysis::chat::{scene_entries, speaker_turns};

/// A sound file this long or longer is music on the library's tabs, unless the timeline plays it as a sound effect (the
/// page's `MUSIC_MIN_US`, `features/media`).
pub const MUSIC_MIN_US: Us = 15_000_000;
/// The ids of the pack's files in a project start with this: they are listed with the pack, not as the project's own.
const PACK_PREFIX: &str = "klipolo-pack-";

/// The context the model reads for `project`, and the inputs its calls compile against: `message` (the times it says
/// are evidence) and `view` (the playhead, the selection) as sent.
pub(crate) fn gather(
    project: &Project,
    findings: &dyn Findings,
    message: &str,
    view: View,
    pack: Vec<LibraryItem>,
) -> (ChatContext, Inputs) {
    let (inputs, turns) = inputs_for(project, findings, message, view, pack);
    let context = build_context(
        project,
        &inputs.speech,
        &turns,
        &inputs.scenes,
        &inputs.view,
        &inputs.library,
    );
    (context, inputs)
}

/// The inputs calls on `project` compile against, as `gather` builds them for a message (its context aside), with each
/// file's speaker turns by asset id: what the analysis found, the library, `message` (the times it says are evidence;
/// empty for another app's calls) and `view` (the person's playhead and selection).
pub fn inputs_for(
    project: &Project,
    findings: &dyn Findings,
    message: &str,
    view: View,
    pack: Vec<LibraryItem>,
) -> (Inputs, HashMap<String, Vec<SpeakerTurn>>) {
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
    let library = library_of(project, pack);
    let inputs = Inputs {
        message: message.to_owned(),
        view,
        refs: Refs::of(project),
        speech,
        scenes,
        analysis,
        time_again: HashSet::new(),
        library,
    };
    (inputs, turns)
}

/// The media chat's tools may name, as the library lists them: `project`'s own music, sounds and pictures, then `pack`.
/// A sound file is music when the timeline plays it as music, or doesn't play it as a sound effect and it is long
/// enough to be a song; a sound effect the other way round; both when it plays as both.
fn library_of(project: &Project, pack: Vec<LibraryItem>) -> Library {
    let (mut music, mut sounds) = (HashSet::new(), HashSet::new());
    for item in project.timeline.tracks.iter().flat_map(|t| &t.items) {
        if let ItemContent::Audio { asset_id, .. } = &item.content {
            if is_music(item) {
                music.insert(asset_id.as_str());
            } else {
                sounds.insert(asset_id.as_str());
            }
        }
    }
    let mut items = Vec::new();
    let own = (project.assets.iter()).filter(|a| !a.id.starts_with(PACK_PREFIX));
    for asset in own {
        let id = asset.id.as_str();
        let item = |kind| LibraryItem {
            name: asset.name.clone(),
            kind,
            asset: asset.clone(),
            pack: false,
        };
        match asset.kind {
            AssetKind::Audio => {
                let long = asset.duration_us >= MUSIC_MIN_US;
                if music.contains(id) || (!sounds.contains(id) && long) {
                    items.push(item(MediaKind::Music));
                }
                if sounds.contains(id) || (!music.contains(id) && !long) {
                    items.push(item(MediaKind::Sound));
                }
            }
            AssetKind::Image => items.push(item(MediaKind::Image)),
            AssetKind::Video => {}
        }
    }
    items.extend(pack);
    Library { items }
}
