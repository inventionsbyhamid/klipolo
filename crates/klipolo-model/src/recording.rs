//! Recordings in format 4: a list of sources, each its own file(s) and lane, with runs where a heal
//! started a new file, plus markers. Everything here is on the recording clock: the recorder's session clock, host
//! time since the start gate with pauses left out, or for a recording Sync made, the first picture file's clock.
//! Re-exported from `project`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::cursor_style::RecordingCursor;
use crate::ids::Id;
use crate::synced::Synced;
use crate::time::Us;
use crate::timeline::EffectInstance;

pub mod chapters;
mod gaps;
mod marker;
pub use marker::{GameInfo, Marker, MarkerKind};

fn is_zero(v: &Us) -> bool {
    *v == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RecordingKind {
    Screen,
    Camera,
    Gameplay,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SourceRole {
    Screen,
    Game,
    Camera,
    Mic,
    SystemAudio,
    GameAudio,
    VoiceChat,
    OtherApp,
}

impl SourceRole {
    /// A source with pictures (its sound, if any, rides along); the others are sound only.
    pub fn is_video(self) -> bool {
        matches!(self, Self::Screen | Self::Game | Self::Camera)
    }

    /// Whether a recording may hold several sources of this role: two lavs, or several apps' sound. Every
    /// other role is one per recording (`Recording::camera` would make a second camera the bubble).
    pub fn may_repeat(self) -> bool {
        matches!(self, Self::Mic | Self::OtherApp)
    }
}

/// How a source plays: its default for every clip, or one clip's override (`Clip::source_mix`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceMix {
    /// 0 to 2 (200 %).
    pub volume: f64,
    pub muted: bool,
    /// Audio effects, in order.
    pub effects: Vec<EffectInstance>,
    /// Play only this channel of the source's files (0 is the first), on both sides at the level a one-channel file
    /// plays at; None plays them as recorded. A mic on one input of a two-input interface records [voice, silence]
    /// import sets this when the other channel stayed silent for the whole take. The files keep
    /// every channel, and a one-channel file plays centred either way. Under `validate::PLAYED_CHANNELS`; left out of
    /// the JSON while None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub centre_from: Option<u8>,
    /// A muted picture source's sound plays where none of the recording's sound sources has a run: the
    /// camera's own sound fills the stretches a recorder didn't record (`Recording::true_gaps`). Only a picture source
    /// whose files carry sound has it (`validate`); Sync turns it on for the camera's sound it keeps muted. Left out of
    /// the JSON while false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fills_gaps: bool,
    /// The person muted it (`set_muted`: chat, a lane, a clip), not Sync (its camera sound, muted, filling the gaps): the
    /// the person's mute wins, so `fillGaps` never fills it. Unmuting clears it; left out while false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted_by_person: bool,
}

impl Default for SourceMix {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
            effects: Vec::new(),
            centre_from: None,
            fills_gaps: false,
            muted_by_person: false,
        }
    }
}

/// One file of a source. It starts `offset_us` into the recording and holds `duration_us` of its own time from
/// `file_in_us` on; its device clock ran `drift_ppm` fast against the recording clock (the recorder or Sync fits it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceRun {
    pub asset_id: Id,
    pub offset_us: Us,
    pub duration_us: Us,
    pub drift_ppm: f64,
    /// Where in its file the run starts (a sound that started before the recording's zero, or after a pause). Left out
    /// of the JSON while 0, as on every take Klipolo records.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub file_in_us: Us,
}

impl SourceRun {
    fn rate(&self) -> f64 {
        1.0 + self.drift_ppm * 1e-6
    }

    /// The run's own time at recording time `rec_us`: ((rec − nudge − offset) × (1 + ppm·10⁻⁶)) + file_in, rounded.
    pub fn local_us(&self, rec_us: Us, nudge_us: Us) -> Us {
        let since = rec_us
            .saturating_sub(nudge_us)
            .saturating_sub(self.offset_us);
        ((since as f64 * self.rate()).round() as Us).saturating_add(self.file_in_us)
    }

    /// The recording time where the run plays its file time `file_us` (the inverse of `local_us`): `useRecording` maps
    /// a clip's file times onto the recording clock with it.
    pub fn rec_of(&self, file_us: Us, nudge_us: Us) -> Us {
        let since = file_us.saturating_sub(self.file_in_us);
        ((since as f64 / self.rate()).round() as Us)
            .saturating_add(self.offset_us)
            .saturating_add(nudge_us)
    }

    /// [from, to) on the recording clock after the nudge (the duration shrinks by the drift).
    pub fn rec_span(&self, nudge_us: Us) -> (Us, Us) {
        let from = self.offset_us.saturating_add(nudge_us);
        let len = (self.duration_us as f64 / self.rate()).round() as Us;
        (from, from.saturating_add(len))
    }
}

/// One source of a recording: its own file(s) and lane. A clean take has one run; a reattached device, a rebuilt
/// a rebuilt encoder or a resumed recorder starts another. Runs lie on the recording clock (the recorder's session clock:
/// host time since the start gate, pauses left out), in order and never overlapping.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecordingSource {
    /// Unique within its recording.
    pub id: Id,
    pub role: SourceRole,
    /// A `capture.source.<role>` code for a name Klipolo made ("capture.source.mic"), or the person's own text once they
    /// they rename it. The UI words a code with `wordCode`; chat's summary words it in English.
    pub label: String,
    pub runs: Vec<SourceRun>,
    /// The person's sync correction, added to every run's offset (±5 s); the recorded offsets stay as measured.
    pub nudge_us: Us,
    /// The default mix; a clip may override it (`Clip::source_mix`).
    pub mix: SourceMix,
    /// How Sync lined this source up; None for a take Klipolo recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub synced: Option<Synced>,
}

impl RecordingSource {
    /// The run playing at recording time `rec_us` and its own time there; None in a gap (sound plays nothing there).
    pub fn run_at(&self, rec_us: Us) -> Option<(&SourceRun, Us)> {
        self.runs.iter().find_map(|r| {
            let (from, to) = r.rec_span(self.nudge_us);
            (from..to)
                .contains(&rec_us)
                .then(|| (r, r.local_us(rec_us, self.nudge_us)))
        })
    }

    /// [`Self::run_at`], else the nearest run and its nearest frame: video holds a frame across a gap.
    pub fn run_or_nearest(&self, rec_us: Us) -> Option<(&SourceRun, Us)> {
        if let Some(hit) = self.run_at(rec_us) {
            return Some(hit);
        }
        // The nearest run by distance on the recording clock; a tie keeps the earlier run (holds its last frame):
        // `min_by_key` returns the first minimum, and runs are ascending (validation keeps them so).
        self.runs
            .iter()
            .min_by_key(|r| {
                let (from, to) = r.rec_span(self.nudge_us);
                if rec_us < from {
                    from.saturating_sub(rec_us)
                } else {
                    rec_us.saturating_sub(to)
                }
            })
            .map(|r| {
                let local = r.local_us(rec_us, self.nudge_us);
                (
                    r,
                    local.clamp(r.file_in_us, r.file_in_us.saturating_add(r.duration_us)),
                )
            })
    }

    /// Where the source's last run ends on the recording clock.
    pub fn end_us(&self) -> Us {
        self.runs
            .iter()
            .map(|r| r.rec_span(self.nudge_us).1)
            .max()
            .unwrap_or(0)
    }
}

/// Files captured together, or lined up by Sync: one source per role, but `Mic` and `OtherApp` may repeat
/// (`SourceRole::may_repeat`). Clip source times are recording times.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Recording {
    pub id: Id,
    pub kind: RecordingKind,
    pub sources: Vec<RecordingSource>,
    /// The source drawn as the main footage: a screen, game or camera source.
    pub primary: Id,
    /// The cursor the recorder kept (screen recordings): its file, how the frames show it, how clicks came. A
    /// project saved before this record named only the file, `cursorPath`; `load_project` reads it into this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<RecordingCursor>,
    /// Sorted by time.
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub game: Option<GameInfo>,
}

impl Recording {
    pub fn source(&self, id: &str) -> Option<&RecordingSource> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn primary_source(&self) -> Option<&RecordingSource> {
        self.source(&self.primary)
    }

    /// The cursor file, when the take kept one.
    pub fn cursor_path(&self) -> Option<&str> {
        self.cursor.as_ref().map(|c| c.path.as_str())
    }

    /// The primary, when it is a screen or a game.
    pub fn screen(&self) -> Option<&RecordingSource> {
        self.primary_source()
            .filter(|s| matches!(s.role, SourceRole::Screen | SourceRole::Game))
    }

    /// The camera is the main footage: a Camera only take, or one whose screen didn't start ("Record without it").
    pub fn camera_only(&self) -> bool {
        self.primary_source()
            .is_some_and(|s| s.role == SourceRole::Camera)
    }

    /// The first camera source that isn't the primary (the bubble).
    pub fn camera(&self) -> Option<&RecordingSource> {
        self.sources
            .iter()
            .find(|s| s.role == SourceRole::Camera && s.id != self.primary)
    }

    /// The sound-only sources, in order. What a clip is heard from also takes in a picture source whose files carry
    /// sound (`query::sound_sources`).
    pub fn audio_sources(&self) -> impl Iterator<Item = &RecordingSource> {
        self.sources.iter().filter(|s| !s.role.is_video())
    }

    /// Every run's file, source by source.
    pub fn asset_ids(&self) -> impl Iterator<Item = &Id> {
        self.sources
            .iter()
            .flat_map(|s| s.runs.iter().map(|r| &r.asset_id))
    }

    /// The latest end of any source.
    pub fn duration_us(&self) -> Us {
        self.sources
            .iter()
            .map(RecordingSource::end_us)
            .max()
            .unwrap_or(0)
    }
}
