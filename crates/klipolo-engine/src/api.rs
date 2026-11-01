//! Types the UI sees (ts-rs → apps/desktop/src/bindings). Engine events travel over a Tauri Channel.

use klipolo_model::project::FormatKind;
use klipolo_model::time::Us;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What `engine_attach` returns: where to connect for frames, and how many pixels a preview frame may have.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EngineInfo {
    pub port: u16,
    pub token: String,
    pub max_preview_pixels: u32,
    pub adapter: String,
    /// How frames reach the screen: `native` (a layer under the transparent webview; the UI only sends the preview
    /// hole's rect) or `socket` (frames arrive over the websocket and the UI draws them).
    pub presenter: Presenter,
    /// What kind of GPU the preview runs on (capabilities are data). Editing copies follow it.
    pub gpu_class: crate::platform::gpu::GpuClass,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Presenter {
    Native,
    #[default]
    Socket,
}

/// Where the native preview goes: the UI's preview hole in the page's coordinates (CSS px at page zoom 1, top-left
/// origin; the page may start below the webview's corner, see `present::macos::page_origin`), the display scale, and
/// the stage colour that shows around it while the window resizes (0xRRGGBB).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ViewRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub backdrop: Option<u32>,
}

/// The preview the UI shows: a format at a size in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ViewSpec {
    pub format: FormatKind,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StateReason {
    Play,
    Pause,
    Ended,
    Hidden,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum EngineEvent {
    /// Frames now belong to `session` (a project was opened or switched); frames of other sessions are stale.
    Ready { session: u32 },
    /// Playback started or stopped at `at_us`. `rate`: the speed it plays at (one of `PLAYBACK_RATES`, negative
    /// backward); 1 when stopped.
    State {
        playing: bool,
        at_us: Us,
        reason: StateReason,
        rate: f64,
    },
    /// Shown once per session while frames keep coming, e.g. "This file couldn't be read: intro.mp4".
    Warning { message: String },
    /// The preview can't render at all.
    Error { message: String },
    /// The native presenter put a frame on screen (the socket path's frames carry the same in their trailer). Sent for
    /// every still and the first frames of each run of playback, then about 4 times a second: the UI's clock runs on
    /// its own in between.
    Shown {
        session: u32,
        seq: u32,
        at_us: Us,
        playback: bool,
        /// When the frame was published and when it was presented (ms since the Unix epoch).
        sent_ms: f64,
        shown_ms: f64,
        /// Frames presented since the previous `Shown` (this one included).
        presented: u32,
    },
    /// Playback is falling behind: `LATE_FRAMES` frames within a second were drawn more than a frame's time after they
    /// were due (after a run's first second, while decoders warm up). Said once per run of playback, at `at_us`, the
    /// frame drawn then. Transcription holds for the rest of that playback; the UI needn't act.
    FramesLate { at_us: Us },
    /// The native presenter stopped working: frames come over the socket from now on.
    Presenter { presenter: Presenter },
    /// A file's waveform peaks or thumbnails are ready (or grew): the timeline reloads what it draws of `path`.
    Media {
        path: String,
        kind: crate::media::MediaKind,
    },
    /// Something the user should know, said once and worded by the UI from its code: `AUDIO_DAMAGED`,
    /// `MEDIA_MISSING`.
    Notice { notice: CodedError },
    /// The preview's note for a source whose disk hasn't answered for `WAITING_AFTER` (`WAITING_FOR_DISK`): a quiet line
    /// under the preview while it waits, never a toast. None when the note goes: a frame drew without waiting for it
    /// (its disk answered, or the preview no longer needs it).
    Waiting { notice: Option<CodedError> },
    /// The crash guard, for the app, never the webview: an agent-made effect is about to be
    /// compiled, or drawn for the first time. Neither happens until the app wrote its mark to disk and said so
    /// (`Engine::mark_written`).
    FirstDraw {
        contrib: klipolo_model::contrib::ContribId,
        /// This request's number, which the answer names (`Engine::mark_written`).
        #[ts(type = "number")]
        seq: u64,
    },
    /// The mark `seq` closes: its compile returned, 60 frames or 2 s drew it, or nothing draws it any more. The app
    /// clears it.
    DrewSafely {
        contrib: klipolo_model::contrib::ContribId,
        #[ts(type = "number")]
        seq: u64,
    },
    /// The preview's sampler slowed an agent-made effect down (`to`), as it ran at `ms` a frame against its class's
    /// `budget_ms`: said once a step.
    EffectDegraded {
        contrib: klipolo_model::contrib::ContribId,
        to: crate::render::effects::sampling::Degrade,
        ms: f64,
        budget_ms: f64,
    },
    /// The graphics device was lost with these agent-made effects drawn in the last 60 frames: the app strikes them.
    GpuLost {
        contribs: Vec<klipolo_model::contrib::ContribId>,
    },
}

impl EngineEvent {
    /// One of the crash guard's events, for the app alone (`FirstDraw`, `DrewSafely`, `EffectDegraded`, `GpuLost`).
    pub fn is_guard(&self) -> bool {
        matches!(
            self,
            Self::FirstDraw { .. }
                | Self::DrewSafely { .. }
                | Self::EffectDegraded { .. }
                | Self::GpuLost { .. }
        )
    }
}

/// The speeds playback runs at (J/K/L shuttle): backward and forward at normal and double speed. Sound plays at
/// 1× only.
pub const PLAYBACK_RATES: [f64; 4] = [-2.0, -1.0, 1.0, 2.0];

/// The code `play` refuses a speed outside `PLAYBACK_RATES` with (the UI words it: "That playback speed isn't
/// available."). The engine's codes are worded from the page's table (`apps/desktop/src/bridge/call.ts`), as `engine.*`.
pub const RATE_UNAVAILABLE: &str = "engine.play.rateUnavailable";
/// `play` with no project open ("Open a project to play it.").
pub const PLAY_NO_PROJECT: &str = "engine.play.noProject";
/// `play` of a project with nothing on its timeline ("Add a video to play it.").
pub const PLAY_NO_VIDEO: &str = "engine.play.noVideo";
/// The engine's actor has ended (a panic): the preview's Retry starts a new one ("The preview has stopped.").
pub const PREVIEW_STOPPED: &str = "engine.stopped";

/// The notice for a file whose sound can't be read past a point (a bad sector, a failing drive): it plays up to there
/// and is silent after it until the project is opened again. Params: `file` (its name) and `atUs` (where its sound
/// stops, in the file's own time).
pub const AUDIO_DAMAGED: &str = "engine.audioDamaged";

/// The notice for a file the preview needs that isn't there (moved, renamed, its drive removed), said once per session:
/// the code an editing copy's row uses for its missing source (the app's `copies.failed.missing`), so both say
/// "intro.mp4 is missing". Params: `name` (its file name).
pub const MEDIA_MISSING: &str = "copies.failed.missing";

/// `MEDIA_MISSING` for the file at `path`.
pub fn missing_notice(path: &str) -> CodedError {
    let mut notice = CodedError::new(MEDIA_MISSING);
    notice.params.insert(
        "name".into(),
        ErrorParam::Text(crate::log_name(path).into_owned()),
    );
    notice
}

/// The preview's note for a source whose disk hasn't answered for `WAITING_AFTER` (asleep, a network volume gone):
/// "Waiting for the disk with intro.mp4…". Neutral: a disk spinning up answers within the pool's stat limit and is
/// never said, and the note goes when it answers (`EngineEvent::Waiting`). Params: `name` (its file name).
pub const WAITING_FOR_DISK: &str = "engine.waitingForDisk";

/// A frame's notes when it couldn't draw everything (codes the app words). The
/// preview says each once as a notice; an export stops on any of them (`exports.frame.*`).
/// - some layers were drawn without their effects (the frame's effect budget);
/// - a layer under a blur, pixelation or mask whose protection couldn't run is hidden;
/// - some texts were drawn softer (the text ceiling).
pub const FRAME_EFFECTS_LEFT_OUT: &str = "engine.frame.effectsLeftOut";
pub const FRAME_PROTECTION_HIDDEN: &str = "engine.frame.protectionHidden";
pub const FRAME_TEXTS_SOFTER: &str = "engine.frame.textsSofter";
/// A text's font isn't on this computer, so it is drawn in Inter (the stand-in). An export stops on it
/// unless its job allows stand-ins (`ExportJob::allow_stand_ins`, the export's "Export with Inter instead"). The font is
/// named by the missing-fonts check, not here: a frame's notes are bare codes.
pub const FRAME_FONT_MISSING: &str = "frame.fontMissing";
/// Chinese, Japanese or Korean text whose Noto Sans isn't downloaded draws Adobe NotDef's boxes, never a system face
/// the person didn't pick. An export always stops on it, even allowing stand-ins: CJK never ships as boxes.
pub const FRAME_CJK_FONT_MISSING: &str = "frame.cjkFontMissing";
/// The captions' style isn't installed (a pack's), so they are drawn in Clean (captions never silently
/// disappear, and the note names the style). The note carries the style's id after the code (`caption_style_missing`);
/// `frame_notice` makes it the notice `{ style, fallback }`. An export stops on it.
pub const FRAME_CAPTION_STYLE_MISSING: &str = "frame.captionStyleMissing";
/// A motion graphic past the runtime caps, once per scene: shapes or particles past them are left out;
/// one that would read too many animated values a frame shows its still. An export draws it the same and says so when
/// it finishes (`EXPORT_SCENE_*` with the first time, in `ExportEvent::Finished`'s notes).
pub const FRAME_SCENE_REDUCED: &str = "frame.sceneReduced";
pub const FRAME_SCENE_STILL: &str = "frame.sceneStill";
/// A path in a motion graphic too detailed to draw (past 20,000 points once flattened, or past the path meshes' room),
/// left out, once per scene.
pub const FRAME_SCENE_TOO_DETAILED: &str = "frame.sceneTooDetailed";
/// A motion graphic's text past its caps (500 characters a text, 2,000 a scene, a cluster of 32), drawn
/// cut, once per scene; an export reports it, as `FRAME_SCENE_REDUCED`.
pub const FRAME_SCENE_TEXT_CUT: &str = "frame.sceneTextCut";
/// A motion graphic's text couldn't be built (its build thread failed to start or stopped), once per scene: the preview
/// draws the scene without it, or with an earlier text in its place, and says
/// so; an export never draws it without, and stops (`EXPORT_SCENE_TEXT_MISSING`).
pub const FRAME_SCENE_TEXT_MISSING: &str = "frame.sceneTextMissing";
/// A motion graphic drew more nodes offscreen at once than Klipolo does (4 groups and 4 other nodes a
/// frame): the rest drew in place without their blur, blend, mask, tilt, opacity or shadow, once per scene; an export
/// reports it, as `FRAME_SCENE_REDUCED`.
pub const FRAME_SCENE_TOO_MANY_GROUPS: &str = "frame.sceneTooManyGroups";
/// "Checking an effect…": a library or project copy the frame uses that this computer hasn't checked
/// yet, or whose settings draw clamped while they are measured again. It draws as nothing (or clamped) and never stops
/// an export (the job leaves out what the preview wouldn't draw). The note carries the id after the code
/// (`agent_effect_checking`), so each is said once.
pub const FRAME_AGENT_EFFECT_CHECKING: &str = "frame.agentEffectChecking";
/// This computer couldn't check an effect a frame uses (the checker refused or broke on it, or answered for another
/// definition): it draws as nothing until the person asks to check it again, or reopens the project.
pub const FRAME_AGENT_EFFECT_CHECK_FAILED: &str = "frame.agentEffectCheckFailed";
/// A project's copy of an effect that doesn't read as saved (a file changed, an extra file, a link, too large): it
/// draws as nothing, and the project still opens (the app's snapshot says it, naming the folder).
pub const FRAME_AGENT_EFFECT_UNREADABLE: &str = "frame.agentEffectUnreadable";
/// The project uses agent-made effects, and the "Agent-made effects" switch is off: they draw as nothing, in the preview
/// and the export.
pub const FRAME_AGENT_EFFECTS_OFF: &str = "frame.agentEffectsOff";
/// An agent-made effect ran far over its budget in the preview, so it is drawn more simply there (at half size, then as
/// a still or without it): said once per effect, with the id after the code, until the project closes.
/// The export draws it in full.
pub const FRAME_AGENT_EFFECT_SLOW: &str = "frame.agentEffectSlow";

/// What stops an export at a frame that couldn't be drawn whole (a degraded frame is never written), each with its
/// time (`atUs`): too many effects, a protection that couldn't run, too many large texts, or another of the frame's
/// notes (`reason`, the engine's English for it).
pub const EXPORT_TOO_MANY_EFFECTS: &str = "exports.frame.tooManyEffects";
pub const EXPORT_PROTECTION_FAILED: &str = "exports.frame.protectionFailed";
pub const EXPORT_TOO_MANY_TEXTS: &str = "exports.frame.tooManyTexts";
pub const EXPORT_FRAME_NOT_DRAWN: &str = "exports.frame.other";
/// What stops an export at a text in a font that isn't here (`FRAME_FONT_MISSING`), or CJK without its Noto Sans
/// (`FRAME_CJK_FONT_MISSING`), each with its time (`atUs`).
pub const EXPORT_FONT_MISSING: &str = "exports.frame.fontMissing";
/// `EXPORT_FONT_MISSING` naming the fonts that frame draws in a stand-in (`names`).
pub const EXPORT_FONT_MISSING_NAMED: &str = "exports.frame.fontMissingNamed";
pub const EXPORT_CJK_FONT_MISSING: &str = "exports.frame.cjkFontMissing";
/// What stops an export at captions whose style isn't installed (`FRAME_CAPTION_STYLE_MISSING`), with its time (`atUs`).
pub const EXPORT_CAPTION_STYLE_MISSING: &str = "exports.frame.captionStyleMissing";
/// A take whose frames have no cursor has a cursor the export can't read: the export asks before
/// exporting without it (`recording`, the recording's name); with the person's yes, each such recording's note.
pub const EXPORT_CURSOR_MISSING: &str = "exports.cursorMissing";
pub const EXPORT_CURSOR_DROPPED: &str = "exports.note.cursorDropped";
/// The same for a take whose file reads but that the takes before it fill the cursor budget for.
pub const EXPORT_CURSOR_OVER_BUDGET: &str = "exports.cursorOverBudget";
pub const EXPORT_CURSOR_OVER_BUDGET_DROPPED: &str = "exports.note.cursorOverBudget";
/// Before its first frame an export analyses the screens its plans read: how the UI words
/// `ExportEvent::Analysing` (`percent`, in the language's format), and the question it asks when a screen can't be read
/// now (`file`, its name): Try again, or export without that screen's activity.
pub const EXPORT_ANALYSING: &str = "exports.analysing";
pub const EXPORT_ACTIVITY_UNREADABLE: &str = "exports.activityUnreadable";
pub const EXPORT_SCENE_REDUCED: &str = "exports.frame.sceneReduced";
pub const EXPORT_SCENE_STILL: &str = "exports.frame.sceneStill";
pub const EXPORT_SCENE_TOO_DETAILED: &str = "exports.frame.sceneTooDetailed";
pub const EXPORT_SCENE_TEXT_CUT: &str = "exports.frame.sceneTextCut";
pub const EXPORT_SCENE_TEXT_MISSING: &str = "exports.frame.sceneTextMissing";
pub const EXPORT_SCENE_TOO_MANY_GROUPS: &str = "exports.frame.sceneTooManyGroups";

/// Whether a frame's note is one of the coded ones (`FRAME_*`), said as a notice rather than as English.
pub fn is_frame_code(note: &str) -> bool {
    matches!(
        note.split_once(' ').map_or(note, |(code, _)| code),
        FRAME_EFFECTS_LEFT_OUT
            | FRAME_PROTECTION_HIDDEN
            | FRAME_TEXTS_SOFTER
            | FRAME_FONT_MISSING
            | FRAME_CJK_FONT_MISSING
            | FRAME_CAPTION_STYLE_MISSING
            | FRAME_SCENE_REDUCED
            | FRAME_SCENE_STILL
            | FRAME_SCENE_TOO_DETAILED
            | FRAME_SCENE_TEXT_CUT
            | FRAME_SCENE_TEXT_MISSING
            | FRAME_SCENE_TOO_MANY_GROUPS
            | FRAME_AGENT_EFFECT_CHECKING
            | FRAME_AGENT_EFFECT_SLOW
    )
}

/// The frame's note for contribution `id`, which the preview's sampler slowed down: the code, a space, then the id.
pub fn agent_effect_slow(id: &str) -> String {
    format!("{FRAME_AGENT_EFFECT_SLOW} {id}")
}

/// The frame's note for contribution `id`, which is being checked: the code, a space, then the id.
pub fn agent_effect_checking(id: &str) -> String {
    format!("{FRAME_AGENT_EFFECT_CHECKING} {id}")
}

/// Whether `note` says a contribution is being checked (`agent_effect_checking`): an export draws the frame without it.
pub fn is_agent_effect_checking(note: &str) -> bool {
    note.split_once(' ').map_or(note, |(code, _)| code) == FRAME_AGENT_EFFECT_CHECKING
}

/// The frame's note for captions whose style `id` isn't installed: the code, a space, then the id (codes and ids hold
/// no spaces).
pub fn caption_style_missing(id: &str) -> String {
    format!("{FRAME_CAPTION_STYLE_MISSING} {id}")
}

/// A frame's coded note as the notice the app words: the code, with the missing caption style named (`style`) and the
/// style drawn instead (`fallback`, worded by its catalogue name). None for a note in English.
pub fn frame_notice(note: &str) -> Option<CodedError> {
    if !is_frame_code(note) {
        return None;
    }
    Some(match note.split_once(' ') {
        // "Checking an effect…" names no effect: the id only keeps each one's note apart.
        Some((FRAME_AGENT_EFFECT_CHECKING, _)) => CodedError::new(FRAME_AGENT_EFFECT_CHECKING),
        // Neither names its effect: the id only keeps each one's note apart.
        Some((FRAME_AGENT_EFFECT_SLOW, _)) => CodedError::new(FRAME_AGENT_EFFECT_SLOW),
        Some((code, id)) => CodedError::new(code)
            .with("style", id)
            .with("fallback", klipolo_model::captions::styles::FALLBACK_STYLE),
        None => CodedError::new(note),
    })
}

/// How long a frame waits for a source's disk before the preview says so (`WAITING_FOR_DISK`).
pub const WAITING_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

/// `WAITING_FOR_DISK` for the file at `path`.
pub fn waiting_notice(path: &str) -> CodedError {
    let mut notice = CodedError::new(WAITING_FOR_DISK);
    notice.params.insert(
        "name".into(),
        ErrorParam::Text(crate::log_name(path).into_owned()),
    );
    notice
}

/// Whether playback runs at `rate` (one of `PLAYBACK_RATES`).
pub fn rate_supported(rate: f64) -> bool {
    PLAYBACK_RATES.contains(&rate)
}

/// A user-facing error as a stable code and the values its message needs, shared with the model.
pub use klipolo_model::coded::{CodedError, ErrorParam, Said};

/// Export quality. High is the default: 1.75 × YouTube's recommended bitrate with B-frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportQuality {
    /// YouTube's recommended SDR bitrates, H.264 High without B-frames: smaller files that upload well.
    Standard,
    /// 1.75 × Standard, H.264 High with two B-frames.
    #[default]
    High,
    /// 3 × Standard, HEVC Main 10 where the Mac has it (H.264 High otherwise).
    Maximum,
    /// ProRes 422 HQ in a .mov with 24-bit PCM sound, for another editor.
    Master,
}

/// What the UI asks to export: one file per format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportRequest {
    pub formats: Vec<FormatKind>,
    #[serde(default)]
    pub quality: ExportQuality,
    /// Also save the captions as this kind of file beside each landscape video; none when not asked.
    #[serde(default)]
    pub caption_file: Option<klipolo_model::captions::sidecar::CaptionFile>,
    /// "Export with Inter instead": a font that isn't here draws its stand-in rather than refusing the
    /// export. The app refuses CJK text without its Noto Sans whatever this says.
    #[serde(default)]
    pub allow_stand_ins: bool,
    /// Start over: the unfinished job this export replaces. The app discards it only once the new export
    /// holds the slot and is prepared, so a start refused on the way keeps the job and its finished parts.
    #[serde(default)]
    pub replaces: Option<String>,
    /// Export without the cursors that can't be read: the person answered `exports.cursorMissing`'s question with
    /// "Export anyway". Each such recording then says so (`exports.note.cursorDropped`).
    #[serde(default)]
    pub allow_missing_cursor: bool,
}

/// One file an export will write: its format, size and final path (`name` is the file name shown to the user).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlannedFile {
    pub format: FormatKind,
    pub width: u32,
    pub height: u32,
    pub name: String,
    pub path: String,
}

/// Export progress, sent to the UI over a Tauri Channel. Frame counts add up every format's frames (two formats of a
/// 900-frame video: 1800), in `Started`, `Progress` and `ExportJobInfo` alike.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ExportEvent {
    /// `job_id` names the job for `export_pause`, `export_resume` and `export_discard`. A resumed job starts with
    /// the frames it already has (`Progress` follows at once).
    Started {
        job_id: String,
        files: Vec<PlannedFile>,
        total_frames: u64,
        segments_total: u32,
    },
    /// `eta_s` is measured, never guessed: None until this run has rendered for 5 s. A segment is a
    /// finished, kept part of one file (about 30 s of one format).
    Progress {
        frame: u64,
        total_frames: u64,
        eta_s: Option<f64>,
        segments_done: u32,
        segments_total: u32,
    },
    /// Something this export does that the person should know, as a code: a recording it exports without its cursor
    /// (`exports.note.cursorDropped`, with the recording's name as `recording`).
    Note { message: Said },
    /// Before the first frame, the screens' activity its plans read is being made ("Analysing the
    /// screen…"): `done` of it, 0–1, sent as each whole percent is reached. The first `Progress` ends it.
    Analysing { done: f32 },
    /// Every part is rendered; they are being joined into the finished files (no time left is shown).
    Finishing,
    /// The worker stopped unexpectedly; it continues from the parts already finished (in hardware again, then in
    /// the slower compatibility mode).
    Retrying,
    /// File `index` of `Started.files` is in place at `path` (a name taken since planning gets the next free one). An
    /// index past them is a caption file: `files.len() + i` is the one beside file `i`.
    FileReady { index: u32, path: String },
    /// File `index`'s caption file couldn't be saved (`disk_full`: the disk is full); the video is in place and the
    /// export finishes.
    CaptionFileFailed { index: u32, disk_full: bool },
    /// A caption file was asked for, but none of the transcribed words are in the video (all cut, hidden or in clips it
    /// doesn't play): there was nothing to save, and the export finishes with its videos.
    CaptionFileEmpty,
    /// Every file is in place; `notes` says what it drew reduced (a motion graphic past its caps), with when (left out
    /// when there is nothing to say).
    Finished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        notes: Option<Vec<Said>>,
    },
    /// Stopped with every finished part kept: the user paused, or `message` says why (a full disk, a hardware
    /// failure: English from the engine, which the app may say as its own code). `export_resume` continues from there;
    /// `compatibility` is set when it may also finish in compatibility mode (the hardware path kept failing).
    Paused {
        message: Option<Said>,
        compatibility: Option<ExportCompatibility>,
    },
    /// The export can't go on (`message` says why, such as a missing file: English, or a code such as
    /// `exports.frame.tooManyEffects`). Its finished parts are kept: the job stays in `export_jobs` and resumes once the
    /// cause is fixed.
    Failed { message: Said },
    /// Stopped and discarded: nothing is left behind.
    Cancelled,
}

/// Finishing a paused export in compatibility mode (VideoToolbox's software encoder, FFmpeg decoding): slower, and it
/// redoes `parts_to_redo` finished parts that were made by the hardware encoder and can't join the new ones.
/// `chosen`: the user already chose it, so Resume continues in it (the UI shows that one action, not both).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportCompatibility {
    pub parts_to_redo: u32,
    pub chosen: bool,
}

/// How `export_resume` continues: `auto` (each format with the encoder its finished parts came from, hardware for a
/// new one) or `compatible` (compatibility mode for every format, after a hardware failure).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ResumeMode {
    #[default]
    Auto,
    Compatible,
}

/// An export that hasn't finished (`export_jobs`): running, paused, stopped by a crash or quit, or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportJobState {
    Running,
    Paused,
    /// The app or the worker stopped while it ran (a crash, a quit, a restart): it can resume.
    Interrupted,
    Failed,
}

/// Where an editing copy stands: in line, being made, paused (playback, an export, the user, a full disk,
/// a hot computer or a busy encoder), finished, failed, or stopped by the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MediaJobState {
    Waiting,
    Working,
    Paused,
    Done,
    Failed,
    Stopped,
}

/// Who pauses or resumes editing copies (`media_jobs_pause` / `media_jobs_resume`). Each holds copies by itself, so one
/// never undoes the other's pause: `person`, the copies pill (the default); `action`, the action registry's
/// `pauseEditingCopies` (chat, MCP, the command palette). The list says which holds are in force (`MediaJobsHold`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PauseBy {
    #[default]
    Person,
    Action,
}

/// What holds every editing copy now, each until it is lifted (`MediaJobList::held`): `person`, the person's pause
/// from the copies pill; `action`, the action registry's `pauseEditingCopies` (chat, MCP); `autoOff`, automatic copies
/// are turned off (`media_jobs_set_auto`). One side's resume never lifts the other's pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MediaJobsHold {
    Person,
    Action,
    AutoOff,
}

/// One source's editing copy, for the open project (in `MediaJobList`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MediaJobInfo {
    /// The source file, as the project names it (`media_job_stop` and `media_job_retry` take it).
    pub path: String,
    /// Its file name.
    pub name: String,
    pub state: MediaJobState,
    /// 0 to 1, from the frames already copied (earlier runs' included).
    pub progress: f64,
    /// Why the file gets an editing copy (it is 4K, its keyframes are far apart…). None when that isn't known: a file
    /// that couldn't be read.
    pub why: Option<crate::media::proxy::decide::ProxyReason>,
    /// Why it is paused or failed, as a code of the app's table (`appCodes.json`: `copies.paused.*`,
    /// `copies.failed.*`; `name` is the file name where the message has it). None while it waits, works, is done or
    /// was stopped: its state says it all.
    pub reason: Option<CodedError>,
}

/// The open project's editing copies, in the order they are made, and what holds them all (the `media-jobs` event and
/// `media_jobs_list`), so a UI loaded again knows who paused them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MediaJobList {
    pub jobs: Vec<MediaJobInfo>,
    /// Every hold in force, each once, in this order: person, action, autoOff. Empty when nothing holds copies (one
    /// may still be paused by itself, for playback or a full disk: see its `reason`).
    pub held: Vec<MediaJobsHold>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportJobInfo {
    pub job_id: String,
    pub project_id: String,
    pub project_name: String,
    pub quality: ExportQuality,
    pub files: Vec<PlannedFile>,
    pub state: ExportJobState,
    /// Why it stopped (a full disk, a missing file, a frame it couldn't draw whole), when it says.
    pub message: Option<Said>,
    pub frame: u64,
    pub total_frames: u64,
    pub segments_done: u32,
    pub segments_total: u32,
    /// The project changed since this export started (or is gone). It still resumes (finishing the version that was
    /// exported); "Start over with your latest changes" is offered beside it.
    pub stale: bool,
    pub created_ms: u64,
    /// What its folder takes on the disk (its finished parts, sound and manifest): Discard frees it.
    pub bytes: u64,
    /// Set when it paused because the hardware path kept failing: it may finish in compatibility mode.
    pub compatibility: Option<ExportCompatibility>,
}
