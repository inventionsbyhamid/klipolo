//! What the recorder's commands take and give, and what its event carries (the page's recorder store is built on these,
//! through the generated `src/bindings`). The worker's own protocol (`CaptureCommand`, `CaptureEvent`) and the setup
//! (`RecorderSetup`) are klipolo-capture's.

use klipolo_capture::protocol::Event;
use klipolo_model::coded::CodedError;
use klipolo_model::history::Snapshot;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The Record dialog's "What to record". Ids are the ones `RecorderSetup.devices` lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum WhatToRecord {
    /// A whole display, without Klipolo's own windows.
    Screen { display: String },
    /// One app's windows on one display.
    App { app_id: String, display: String },
}

/// One take as the person set it up in the Record dialog. Arming launches the take's worker and captures nothing: only
/// the person's Record press (`recorder_send` with `Start`) starts it. Everything else about the take is the app's
/// (the quality, no computer sound, the shortcuts, the countdown and where the take lands).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArmOptions {
    pub what: WhatToRecord,
    /// A microphone id; None: "No microphone".
    pub mic: Option<String>,
    /// A camera id; None: "No camera".
    pub camera: Option<String>,
    /// "Show my camera while recording": a small round live picture of the camera on the recorded screen, never in the
    /// video. Only with a camera.
    pub camera_bubble: bool,
    /// "Mirror my camera": the bubble and the take's camera layer show the camera as in a mirror. The recorded file
    /// never is.
    pub mirror_camera: bool,
}

/// An armed take: its worker waits for the person's Record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArmedTake {
    /// The take's id: every `recorder_send` and `recorder-event` names it.
    pub id: String,
    /// The countdown after Record, in seconds, before the take records.
    pub countdown_s: u32,
    /// What the person is told before Record: the camera this computer can't record beside the screen
    /// (`capture.preflight.cameraWithScreen`; the take records without it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub notes: Option<Vec<CodedError>>,
}

/// Klipolo opened again just now through Reopen Klipolo (`recorder_back_from_reopen`): the person was choosing what to
/// record, so the page opens the project they had open, then the Record dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BackFromReopen {
    /// The project open as Reopen Klipolo was pressed, by its file; None on the start screen.
    pub project: Option<String>,
}

/// What "Stop the running recording" stopped (`recorder_stop_running`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StoppedTake {
    /// The take's id (`ArmedTake.id`).
    pub id: String,
    /// It had started recording, so it is being saved. False: it was only armed or counting down, so nothing was
    /// recorded and it ends with its `State Idle` alone.
    pub started: bool,
}

/// The `recorder-event` payload: a worker's event, or the app's own about the take (its `Failed`, its notes and its
/// closing `State Idle`), with the take it belongs to, so a late event from an earlier take never reaches a newer one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecorderEvent {
    pub take: String,
    pub event: Event,
}

/// The `recorder-imported` payload: take `take` landed as one undo step ("Add recording", `undo.record`), its clip
/// last on the main track. Sent once the take is saved, before its `State Idle`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TakeImport {
    /// The take's id (`ArmedTake.id`), which is also the recording's.
    pub take: String,
    pub recording_id: String,
    /// The take's clip on the main track.
    pub clip_id: String,
    /// The project it landed in, open now: the one open when the take was armed (opened again when another was open by
    /// then), or a new one ("Recording, Oct 31") for a take from the start screen.
    pub snapshot: Snapshot,
    /// What the person is told about the take: how it ended when that wasn't said already, the recorder's notes, then
    /// the import's own.
    pub notes: Vec<CodedError>,
    /// Auto zoom items added after it, as a step of their own.
    pub zooms: u32,
}

/// A privacy settings page of macOS's, which the recorder's permission steps open ("Open privacy settings").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PrivacyPage {
    Screen,
    Camera,
    Mic,
}

/// What the Record dialog wants previewed while it is open and Klipolo is in front (`recorder_previews`). The app starts
/// and stops its preview workers to match, within the consent rules: screen and app pictures only while Screen
/// Recording is allowed, the camera and the mic only while allowed or on the person's own press (`ask`), and nothing
/// during a take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Previews {
    /// A still of each display and app of the dialog's last setup, about once a second each (`PreviewKind::Thumb`).
    pub thumbs: bool,
    /// The live picture of what a take of this would record, small (`PreviewKind::Live`); None: no live picture.
    pub live: Option<WhatToRecord>,
    /// The camera's picture for the bubble (`PreviewKind::Camera`); None: "No camera".
    pub camera: Option<String>,
    /// The mic's level, ten peaks a second, as `recorder-event`s of `PreviewId::Mic`; None: "No microphone".
    pub mic: Option<String>,
    /// The person just pressed "Allow camera" (`camera`) or "Allow microphone" (`mic`): that preview starts though
    /// macOS hasn't asked yet, so macOS asks now. Only in the call that press makes; None in every other call.
    pub ask: Option<PrivacyPage>,
}

/// Which picture a preview frame is (`PreviewFrame.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PreviewKind {
    /// A display's or an app's card.
    Thumb,
    /// The live picture of what is chosen.
    Live,
    /// The camera's bubble.
    Camera,
}

/// A preview frame's header. Each frame reaches the page over `recorder_previews`'s channel as raw bytes: the header's
/// length in two bytes (big-endian), the header as UTF-8 JSON, then the JPEG's own bytes (`frames::frame_message`). The
/// page acknowledges each one it draws, or drops, with `recorder_preview_ack(kind)`: at most two of a kind are on their
/// way at once, and newer ones are dropped meanwhile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewFrame {
    pub kind: PreviewKind,
    /// What it shows: a card's key (`display:<DisplayInfo.id>` or `app:<AppInfo.appId>`), the same key for the live
    /// picture of that choice, or the camera's id.
    pub key: String,
    /// When the app got it, in milliseconds since 1970 (as `Date.now()`): a card whose last still is a few seconds old
    /// has stopped getting new ones.
    #[ts(type = "number")]
    pub at_ms: u64,
}

/// The `recorder-event` id of a preview's own events: the mic's peaks (`Meter`), and why a preview stopped by itself
/// (`Failed`). Never a take's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PreviewId {
    #[serde(rename = "preview-thumbs")]
    Thumbs,
    #[serde(rename = "preview-live")]
    Live,
    #[serde(rename = "preview-camera")]
    Camera,
    #[serde(rename = "preview-mic")]
    Mic,
}

impl PreviewId {
    /// The id as `RecorderEvent.take` carries it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thumbs => "preview-thumbs",
            Self::Live => "preview-live",
            Self::Camera => "preview-camera",
            Self::Mic => "preview-mic",
        }
    }
}
