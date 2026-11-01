use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::coded::ErrorParam;
use crate::cursor_style::CursorStyle;
use crate::ids::Id;
use crate::layout::Layout;
use crate::params::ParamValue;
use crate::project::{
    Asset, AudioSettings, Background, CaptionSettings, ColorLut, FormatKind, Marker, Recording,
    RecordingSource, Settings, SourceMix, SourceRun, Synced,
};
pub use crate::risk::{op_risk, OpRisk};
use crate::time::{RangeUs, Us};
use crate::timeline::{
    CameraLayout, Clip, EffectInstance, Framing, Item, ItemContent, Look, TrackKind, Transition,
};

/// One atomic change request. Batches of ops form a [`Transaction`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum Op {
    Rename {
        name: String,
    },
    SetSettings {
        settings: Settings,
    },
    /// Replaces the caption settings. Turning captions on with no style writes the formats' default.
    SetCaptions {
        captions: CaptionSettings,
    },
    /// The words of `asset_id`'s transcript in `[start_us, end_us)` (its sound time) read `text` in captions and the
    /// Script view; `None` removes every fix in that range. The recording itself never changes.
    FixWords {
        asset_id: Id,
        start_us: Us,
        end_us: Us,
        text: Option<String>,
    },
    /// The look of `asset_id`'s words in each of `ranges` (its own time) in captions; an empty look clears them.
    /// Ranges that overlap or touch are as one. A tool or the word list styles every word that takes one look with one
    /// op, applied in one pass over the file's looks, never one op per word. Additive.
    SetWordLook {
        asset_id: Id,
        ranges: Vec<RangeUs>,
        look: crate::captions::looks::Look,
    },
    SetBackground {
        background: Background,
    },
    SetAudio {
        audio: AudioSettings,
    },
    AddAssets {
        assets: Vec<Asset>,
    },
    AddRecording {
        recording: Recording,
    },
    RemoveAsset {
        id: Id,
    },
    AddLut {
        lut: ColorLut,
    },
    RemoveLut {
        id: Id,
    },
    AddTrack {
        id: Option<Id>,
        kind: TrackKind,
        index: Option<usize>,
    },
    RemoveTrack {
        id: Id,
    },
    MoveTrack {
        id: Id,
        index: usize,
    },
    UpdateTrack {
        id: Id,
        patch: TrackPatch,
    },
    /// Insert `clip` into the main track at `index` (clamped; `None` appends).
    InsertClip {
        clip: Clip,
        index: Option<usize>,
    },
    /// Move a main-track clip to `index` (clamped to the end).
    MoveClip {
        id: Id,
        index: usize,
    },
    /// Set a clip's source in/out point. Trimming inward removes that part with a ripple.
    TrimClip {
        id: Id,
        edge: Edge,
        source_us: Us,
    },
    /// Split the main-track clip playing at `at_us` into two.
    Split {
        at_us: Us,
    },
    /// Remove timeline ranges from the main track; gaps close and attached layers follow. Free layers keep their time,
    /// and those left past the video's new end are trimmed to it or removed (music is fitted to it).
    DeleteRanges {
        ranges: Vec<RangeUs>,
    },
    /// Remove a clip and close the gap.
    RemoveClip {
        id: Id,
    },
    /// Change a clip's speed, volume, fades, look or camera; unset patch fields are left alone.
    UpdateClip {
        id: Id,
        patch: ClipPatch,
    },
    /// Add a layer. `attach: true` ties it to the main-track content at its start.
    AddItem {
        track: TrackTarget,
        item: Item,
        attach: bool,
    },
    /// Change a layer's duration, layout or content (content must stay the same kind of layer).
    UpdateItem {
        id: Id,
        patch: ItemPatch,
    },
    /// Move a layer to `start_us`, optionally onto another track; attached layers re-anchor there.
    MoveItem {
        id: Id,
        track_id: Option<Id>,
        start_us: Us,
    },
    /// Move one end of a layer to timeline time `at_us`.
    TrimItem {
        id: Id,
        edge: Edge,
        at_us: Us,
    },
    /// Split a layer in two at timeline time `at_us`.
    SplitItem {
        id: Id,
        at_us: Us,
    },
    RemoveItem {
        id: Id,
    },
    /// Tie a layer to (or free it from) the main-track content at its start.
    SetAttach {
        id: Id,
        attached: bool,
    },
    /// Put one of a recording clip's sources in layers of its own, one per run inside the clip (ids `{clip}-{source}`,
    /// then `-2`, `-3`): the camera as muted video layers, its bubble hidden in every format (what "Separate camera"
    /// did); a sound source as attached sound layers, muted on the clip so it isn't heard twice (only at normal speed).
    /// The main footage stays.
    DetachSource {
        clip_id: Id,
        source_id: Id,
    },
    /// Change a recording source's name, default mix, sync nudge or timing (its runs, re-timed by Sync); unset patch
    /// fields are left alone.
    UpdateRecordingSource {
        recording_id: Id,
        source_id: Id,
        patch: SourcePatch,
    },
    /// Give a recording clip its own mix of one of its sound sources, or go back to the source's (`None`).
    SetClipSourceMix {
        clip_id: Id,
        source_id: Id,
        mix: Option<SourceMix>,
    },
    /// A main-track clip of an imported video plays from `recording_id`, whose primary picture source holds the clip's
    /// file as a run: the same footage at the same place; its source times, and the anchors of layers tied to this
    /// clip, move onto the recording clock. Additive.
    UseRecording {
        clip_id: Id,
        recording_id: Id,
    },
    /// A synced source added to a recording (a recorder's file for a take Klipolo recorded). Additive.
    AddRecordingSource {
        recording_id: Id,
        source: RecordingSource,
    },
    /// Put a marker on a recording (one with the same id is replaced); markers stay sorted by time.
    AddMarker {
        recording_id: Id,
        marker: Marker,
    },
    RemoveMarker {
        recording_id: Id,
        id: Id,
    },
    /// Add `effect` to a stack at `index` (clamped; `None` appends). An empty id becomes `fx`, `fx-2`, … within the
    /// stack.
    AddEffect {
        target: EffectTarget,
        effect: EffectInstance,
        index: Option<usize>,
    },
    /// Switch an effect on or off, or change its settings.
    UpdateEffect {
        target: EffectTarget,
        id: Id,
        patch: EffectPatch,
    },
    RemoveEffect {
        target: EffectTarget,
        id: Id,
    },
    /// Move an effect within its stack (clamped to the end).
    MoveEffect {
        target: EffectTarget,
        id: Id,
        index: usize,
    },
    /// Give a clip a transition into the next clip, or take it away (`None`). Positions don't change.
    SetTransition {
        clip_id: Id,
        transition: Option<Transition>,
    },
    /// The project's cursor style, or clips' own (`None` clears theirs; the project always has one).
    SetCursorStyle {
        target: CursorTarget,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        style: Option<CursorStyle>,
    },
}

/// Whose cursor style [`Op::SetCursorStyle`] sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum CursorTarget {
    Project,
    Clips { ids: Vec<Id> },
}

/// Where [`Op::AddItem`] puts a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum TrackTarget {
    /// First unlocked track of the right kind with room, else a new track on top.
    Auto,
    /// This track, or the next free track above it if the layer doesn't fit.
    Track { id: Id },
}

/// Layer fields to change. Unset fields are left alone and omitted from JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ItemPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub duration_us: Option<Us>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub layout: Option<Layout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub content: Option<ItemContent>,
    /// Replaces the whole effect stack (paste attributes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effects: Option<Vec<EffectInstance>>,
}

/// Which end of a clip or layer a trim moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Edge {
    Start,
    End,
}

/// Clip fields to change. Unset fields are left alone and omitted from JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClipPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub speed: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub volume: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fade_in_us: Option<Us>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fade_out_us: Option<Us>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub look: Option<Look>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub camera: Option<CameraLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub framing: Option<BTreeMap<FormatKind, Framing>>,
    /// Replaces the whole effect stack (paste attributes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effects: Option<Vec<EffectInstance>>,
}

/// Recording source fields to change. Unset fields are left alone and omitted from JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourcePatch {
    /// The person's own name for it (a `capture.source.*` code puts Klipolo's back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mix: Option<SourceMix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nudge_us: Option<Us>,
    /// The same files re-timed (a sync again, a hand line-up); the layers tied to the source follow, so every file is
    /// kept: Additive (a layer cut back where its file ends is judged by review).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub runs: Option<Vec<SourceRun>>,
    /// How it was lined up (with `runs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub synced: Option<Synced>,
}

/// Track fields to change. Unset fields are left alone and omitted from JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TrackPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub locked: Option<bool>,
    /// Overlay and audio tracks only (`Track::solo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub solo: Option<bool>,
}

/// A labelled batch of ops; one undo step. `label` is a message id (`"edit.split"`, `"text.add"`) and `params` its
/// values: the UI words both in the language shown, and chat's planner reads them in English.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Transaction {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub params: Option<BTreeMap<String, ErrorParam>>,
    pub ops: Vec<Op>,
    /// None means the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub origin: Option<Origin>,
}

/// The stack an effect op edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum EffectTarget {
    /// A main-track clip's footage.
    Clip { id: Id },
    /// A layer (not on a locked track).
    Item { id: Id },
}

/// Effect fields to change. Unset fields are left alone and omitted from JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EffectPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    /// Merged into the stored values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub params: Option<BTreeMap<String, ParamValue>>,
    /// Keys to drop, so they take the effect's defaults again (applied before `params`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reset: Option<Vec<String>>,
}

/// Who asked for a change: undo labels, proposal routing and, for plugins, the audit trail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum Origin {
    User,
    Chat,
    Polish,
    Plugin {
        id: String,
        version: String,
    },
    /// An agent connected through the local MCP server, by the name it introduced itself with. Judged by
    /// what it removes, like chat; whether its batches wait for Apply is the app's trust level.
    External {
        client: String,
    },
}

impl Op {
    /// The op's JSON `type` (`"deleteRanges"`): the edit action's id in the registry.
    pub fn tag(&self) -> &'static str {
        match self {
            Op::Rename { .. } => "rename",
            Op::SetSettings { .. } => "setSettings",
            Op::SetCaptions { .. } => "setCaptions",
            Op::FixWords { .. } => "fixWords",
            Op::SetWordLook { .. } => "setWordLook",
            Op::SetBackground { .. } => "setBackground",
            Op::SetAudio { .. } => "setAudio",
            Op::AddAssets { .. } => "addAssets",
            Op::AddRecording { .. } => "addRecording",
            Op::RemoveAsset { .. } => "removeAsset",
            Op::AddLut { .. } => "addLut",
            Op::RemoveLut { .. } => "removeLut",
            Op::AddTrack { .. } => "addTrack",
            Op::RemoveTrack { .. } => "removeTrack",
            Op::MoveTrack { .. } => "moveTrack",
            Op::UpdateTrack { .. } => "updateTrack",
            Op::InsertClip { .. } => "insertClip",
            Op::MoveClip { .. } => "moveClip",
            Op::TrimClip { .. } => "trimClip",
            Op::Split { .. } => "split",
            Op::DeleteRanges { .. } => "deleteRanges",
            Op::RemoveClip { .. } => "removeClip",
            Op::UpdateClip { .. } => "updateClip",
            Op::AddItem { .. } => "addItem",
            Op::UpdateItem { .. } => "updateItem",
            Op::MoveItem { .. } => "moveItem",
            Op::TrimItem { .. } => "trimItem",
            Op::SplitItem { .. } => "splitItem",
            Op::RemoveItem { .. } => "removeItem",
            Op::SetAttach { .. } => "setAttach",
            Op::DetachSource { .. } => "detachSource",
            Op::UpdateRecordingSource { .. } => "updateRecordingSource",
            Op::SetClipSourceMix { .. } => "setClipSourceMix",
            Op::UseRecording { .. } => "useRecording",
            Op::AddRecordingSource { .. } => "addRecordingSource",
            Op::AddMarker { .. } => "addMarker",
            Op::RemoveMarker { .. } => "removeMarker",
            Op::AddEffect { .. } => "addEffect",
            Op::UpdateEffect { .. } => "updateEffect",
            Op::RemoveEffect { .. } => "removeEffect",
            Op::MoveEffect { .. } => "moveEffect",
            Op::SetTransition { .. } => "setTransition",
            Op::SetCursorStyle { .. } => "setCursorStyle",
        }
    }
}

/// Every op's tag, read from the generated schema rather than listed by hand, so the registry's coverage test sees a
/// new variant the moment it exists.
pub fn op_tags() -> Vec<String> {
    let schema = serde_json::to_value(schemars::schema_for!(Op)).expect("the Op schema serialises");
    schema["oneOf"]
        .as_array()
        .expect("Op is an internally tagged enum")
        .iter()
        .filter_map(|variant| {
            let tag = &variant["properties"]["type"];
            tag["const"]
                .as_str()
                .or_else(|| tag["enum"][0].as_str())
                .map(str::to_owned)
        })
        .collect()
}
