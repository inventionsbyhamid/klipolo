//! Which edits can't change what `audio_plan` plans (`may_change_sound`), so a dry run skips planning the sound again.
//! A project with a sound layer that ducks also hears where someone speaks (`speech_ranges`), which the transcript's
//! word fixes can move.

use klipolo_model::Op;
use klipolo_model::ops::EffectTarget;
use klipolo_model::project::Project;
use klipolo_model::query::find_item;
use klipolo_model::timeline::ItemContent;

/// Whether a layer's content makes sound in the mix (a muted video counts: unmuting it is a content change).
fn sounds(content: &ItemContent) -> bool {
    matches!(
        content,
        ItemContent::Audio { .. } | ItemContent::Video { .. }
    )
}

/// Whether `op`, applied to `p`, may change what `audio_plan` plans. False only for ops that
/// can't: the picture alone (settings, captions and the words' looks, background, LUTs, effect stacks of clips and
/// picture layers, a clip's look, camera or framing, a new empty track, a track's name or lock), and edits of layers
/// without sound (titles, images, stickers, zooms, generators, adjustments), which the model places where they fit and
/// never moves another layer for. A sounding layer's layout can't change it either, nor a video layer's effects; a
/// sound layer's effects are audio effects, so they can. A word fix counts when a sound layer ducks: fixing a pause
/// into words makes it speech, which the music goes down under (`speech_ranges`). Everything else counts as a change,
/// including an op on a layer `p` doesn't have (one an earlier op of the batch adds). The engine skips planning the
/// sound again for a dry run whose ops all can't change it: a title drag while playing costs no replan. O(layers) for a
/// layer op (finding the layer) or a word fix (finding a layer that ducks), O(1) otherwise.
pub fn may_change_sound(p: &Project, op: &Op) -> bool {
    let layer_sounds = |id: &str| {
        find_item(p, id).is_none_or(|(t, i)| sounds(&p.timeline.tracks[t].items[i].content))
    };
    // A sound layer's effects are audio effects (a video layer's are pictures).
    let sound_layer = |id: &str| {
        find_item(p, id).is_none_or(|(t, i)| {
            matches!(
                p.timeline.tracks[t].items[i].content,
                ItemContent::Audio { .. }
            )
        })
    };
    match op {
        Op::Rename { .. }
        | Op::SetSettings { .. }
        | Op::SetCaptions { .. }
        | Op::SetWordLook { .. }
        | Op::SetBackground { .. }
        | Op::AddLut { .. }
        | Op::RemoveLut { .. }
        | Op::AddTrack { .. }
        // Markers are pictures on the timeline only, and the cursor is a picture.
        | Op::AddMarker { .. }
        | Op::RemoveMarker { .. }
        | Op::SetCursorStyle { .. } => false,
        // A fix that turns a pause into words moves where someone speaks.
        Op::FixWords { .. } => super::ducks(p),
        // A clip's effects are pictures; a sound layer's are audio effects (a video layer's are pictures).
        Op::AddEffect { target, .. }
        | Op::UpdateEffect { target, .. }
        | Op::RemoveEffect { target, .. }
        | Op::MoveEffect { target, .. } => match target {
            EffectTarget::Clip { .. } => false,
            EffectTarget::Item { id } => sound_layer(id),
        },
        Op::UpdateTrack { patch, .. } => {
            patch.hidden.is_some() || patch.muted.is_some() || patch.solo.is_some()
        }
        Op::UpdateClip { patch, .. } => {
            patch.speed.is_some()
                || patch.volume.is_some()
                || patch.muted.is_some()
                || patch.fade_in_us.is_some()
                || patch.fade_out_us.is_some()
        }
        Op::AddItem { item, .. } => sounds(&item.content),
        Op::UpdateItem { id, patch } => {
            patch.content.as_ref().is_some_and(sounds)
                || (layer_sounds(id) && (patch.duration_us.is_some() || patch.content.is_some()))
                || (patch.effects.is_some() && sound_layer(id))
        }
        Op::MoveItem { id, .. }
        | Op::TrimItem { id, .. }
        | Op::SplitItem { id, .. }
        | Op::RemoveItem { id }
        | Op::SetAttach { id, .. } => layer_sounds(id),
        Op::SetAudio { .. }
        | Op::AddAssets { .. }
        | Op::AddRecording { .. }
        | Op::RemoveAsset { .. }
        | Op::RemoveTrack { .. }
        | Op::MoveTrack { .. }
        | Op::InsertClip { .. }
        | Op::MoveClip { .. }
        | Op::TrimClip { .. }
        | Op::Split { .. }
        | Op::FreezeFrame { .. }
        | Op::DeleteRanges { .. }
        | Op::RemoveClip { .. }
        | Op::DetachSource { .. }
        | Op::DetachSound { .. }
        // A source's mix, nudge or runs re-timed, and Sync's two ops: a clip on its recording now plays the
        // recording's sound, and a new source may sound.
        | Op::UpdateRecordingSource { .. }
        | Op::SetClipSourceMix { .. }
        | Op::UseRecording { .. }
        | Op::AddRecordingSource { .. }
        | Op::SetTransition { .. } => true,
    }
}
