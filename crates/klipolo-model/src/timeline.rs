use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contrib::ContribId;
use crate::cursor_style::{CursorStyle, FocusKind, Follow};
use crate::font::FontRef;
use crate::ids::{new_id, Id};
use crate::layout::{Anchor, BoxN, Layout};
use crate::params::ParamValue;
use crate::project::FormatKind;
use crate::recording::{RecordingSource, SourceMix};
use crate::time::{scaled, Us};

pub mod generator;
use generator::is_zero_us;
pub use generator::{piece_time, seed_id, PieceTime};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Timeline {
    /// Magnetic main track: clips play back to back, no gaps.
    pub main: Vec<Clip>,
    /// Overlay/audio/zoom tracks, bottom to top (later tracks draw on top).
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum ClipSource {
    Asset { id: Id },
    Recording { id: Id },
}

impl ClipSource {
    pub fn id(&self) -> &Id {
        match self {
            Self::Asset { id } | Self::Recording { id } => id,
        }
    }
}

fn enabled() -> bool {
    true
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// One entry of a clip's or layer's effect stack. Effects run in stack order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EffectInstance {
    /// Unique within its stack; split pieces keep their stacks, ids included.
    pub id: Id,
    pub effect: ContribId,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Missing keys take the effect's defaults; keys it doesn't know are kept.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
}

impl EffectInstance {
    /// Enabled, default settings, no id yet (`AddEffect` gives it one).
    pub fn new(effect: impl Into<ContribId>) -> Self {
        Self {
            id: String::new(),
            effect: effect.into(),
            enabled: true,
            params: BTreeMap::new(),
        }
    }

    pub fn with(mut self, key: &str, value: ParamValue) -> Self {
        self.params.insert(key.into(), value);
        self
    }
}

/// A clip's transition into the next clip: `duration_us` centred on the cut, drawn by a transition
/// contribution from the footage past this clip's out point and before the next clip's in point (its handles).
/// Clip positions don't change, so trims and ripples work as before (`transitions::normalize` keeps it fitting).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Transition {
    pub effect: ContribId,
    pub duration_us: Us,
    /// Missing keys take the transition's defaults; keys it doesn't know are kept.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Look {
    /// A look contribution (`klipolo.builtin/look.warm`), or no look.
    pub preset: Option<ContribId>,
    pub strength: f64,
    pub lut_id: Option<Id>,
    pub lut_strength: f64,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            preset: None,
            strength: 1.0,
            lut_id: None,
            lut_strength: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CameraMode {
    /// Floating camera over the screen.
    Bubble,
    /// Screen on top, camera underneath (vertical formats).
    SplitBottom,
    Hidden,
}

/// Corner radius, as a fraction of the shorter side, of a separated rounded camera whose bubble stored none (a layer's
/// `layout.radius`; a circle is 0.5 and a square 0).
pub const ROUNDED_CAMERA_RADIUS: f64 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CameraShape {
    Circle,
    Rounded,
    Square,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CameraLayout {
    pub mode: CameraMode,
    #[serde(default)]
    pub mode_overrides: BTreeMap<FormatKind, CameraMode>,
    pub shape: CameraShape,
    pub border: bool,
    /// Shown as in a mirror (on by default, as the recorder's "Mirror my camera" is). Only the picture is
    /// flipped as it is drawn: the recorded file is the camera's own, so this stays a choice per clip.
    pub mirror: bool,
    pub bubble: Layout,
}

impl CameraLayout {
    pub fn mode_for(&self, format: FormatKind) -> CameraMode {
        self.mode_overrides
            .get(&format)
            .copied()
            .unwrap_or(self.mode)
    }

    /// A new clip's camera in a project whose main format is `primary`: the default, with its bubble a 384 px square
    /// (of 1080 on the format's shorter side) 4 % in from the main format's bottom-right corner. The bubble's box is in
    /// the main format's shares, so sized there it is round in that format and in every format placed from it; the
    /// 16:9 box in a 9:16 project would be a tall pill.
    pub fn for_primary(primary: FormatKind) -> Self {
        let rect = match primary {
            FormatKind::Landscape => BoxN {
                x: 0.76,
                y: 0.6044,
                w: 0.2,
                h: 0.3556,
            },
            FormatKind::Portrait => BoxN {
                x: 0.6044,
                y: 0.76,
                w: 0.3556,
                h: 0.2,
            },
            FormatKind::Square => BoxN {
                x: 0.6044,
                y: 0.6044,
                w: 0.3556,
                h: 0.3556,
            },
        };
        let mut mode_overrides = BTreeMap::new();
        mode_overrides.insert(FormatKind::Portrait, CameraMode::SplitBottom);
        Self {
            mode: CameraMode::Bubble,
            mode_overrides,
            shape: CameraShape::Circle,
            border: true,
            mirror: true,
            bubble: Layout::new(Anchor::BottomRight, rect),
        }
    }
}

/// The camera a clip starts with in a 16:9 project (`CameraLayout::for_primary`).
impl Default for CameraLayout {
    fn default() -> Self {
        Self::for_primary(FormatKind::Landscape)
    }
}

/// Fit shows the whole picture, letterboxed on the background; Fill covers the area and crops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FramingMode {
    Fit,
    Fill,
}

/// How a main-track clip fills one format's canvas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Framing {
    pub mode: FramingMode,
    /// 0 to 1: which part stays in view when Fill crops (0.5 = centre).
    pub focus_x: f64,
    pub focus_y: f64,
    /// The Fill crop follows the cursor; none keeps the focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub follow: Option<Follow>,
}

/// The project feature (`Project::requires`) a main track with a freeze frame needs: a build without freeze frames
/// would play one as moving footage and drop it on save, so it opens such a project read-only.
pub const FREEZE_FEATURE: &str = "freeze@1";
/// The longest freeze frame `FreezeFrame` makes: 10 minutes (trims may make it longer).
pub const MAX_FREEZE_US: Us = 600_000_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Clip {
    pub id: Id,
    pub source: ClipSource,
    pub source_in_us: Us,
    pub source_out_us: Us,
    pub speed: f64,
    pub volume: f64,
    pub muted: bool,
    pub fade_in_us: Us,
    pub fade_out_us: Us,
    pub look: Look,
    /// Only for recording clips that include a camera.
    pub camera: Option<CameraLayout>,
    /// Per-format framing. A format without an entry uses `framing::default_framing`.
    #[serde(default)]
    pub framing: BTreeMap<FormatKind, Framing>,
    /// The main footage's effects, in order. The camera bubble isn't affected.
    #[serde(default)]
    pub effects: Vec<EffectInstance>,
    /// The transition into the next clip; none for a cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub transition_out: Option<Transition>,
    /// A recording clip's own mix of its sound sources, by source id, over each source's default: "mute the game
    /// sound here" follows cuts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_mix: BTreeMap<Id, SourceMix>,
    /// This clip's own cursor style; none draws the project's (`Project::cursor_for`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<CursorStyle>,
    /// A freeze frame (`FreezeFrame`): the clip holds its source's frame at this source time for its whole length (a
    /// recording's every picture and its cursor as they were then) and plays no sound. Its source range only gives its
    /// length and the room its trims have, so it may run past the file, and it holds no footage: its sound, words and
    /// cursor path are nobody's. Left out of the JSON while None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub freeze_at_us: Option<Us>,
    /// The clip plays backward (`ReverseClip`): its source is a copy of this file's stretch written backward, and
    /// Reverse again plays it forward from this file. Left out of the JSON while None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reversed: Option<Reversed>,
}

/// What a reversed clip's copy holds: imported file `of`'s `[from_us, to_us)` written backward, so the copy's time `t`
/// is `of`'s `to_us - t`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Reversed {
    pub of: Id,
    pub from_us: Us,
    pub to_us: Us,
}

/// A copy to play a clip backward from (`ReverseClip`): asset `asset_id`, the clip's file's `[from_us, to_us)` written
/// backward.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReverseCopy {
    pub asset_id: Id,
    pub from_us: Us,
    pub to_us: Us,
}

impl Clip {
    pub fn new(source: ClipSource, source_in_us: Us, source_out_us: Us) -> Self {
        Self {
            id: new_id(),
            source,
            source_in_us,
            source_out_us,
            speed: 1.0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            look: Look::default(),
            camera: None,
            framing: BTreeMap::new(),
            effects: Vec::new(),
            transition_out: None,
            source_mix: BTreeMap::new(),
            cursor: None,
            freeze_at_us: None,
            reversed: None,
        }
    }

    /// Whether the clip is a freeze frame (`freeze_at_us`): it shows one frame and holds no footage.
    pub fn is_freeze(&self) -> bool {
        self.freeze_at_us.is_some()
    }

    /// The mix `source` plays at in this clip: the clip's own override, else the source's default.
    pub fn mix_for<'a>(&'a self, source: &'a RecordingSource) -> &'a SourceMix {
        self.source_mix.get(&source.id).unwrap_or(&source.mix)
    }

    /// Length on the timeline.
    pub fn duration_us(&self) -> Us {
        scaled(
            self.source_out_us.saturating_sub(self.source_in_us),
            self.speed,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TrackKind {
    Overlay,
    Audio,
    Zoom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Track {
    pub id: Id,
    pub kind: TrackKind,
    pub name: String,
    pub hidden: bool,
    pub muted: bool,
    /// Blocks direct edits to this track's items (ripple still applies to keep sync).
    pub locked: bool,
    /// While any track is soloed, only soloed tracks are heard and, of the overlay tracks, only soloed ones are seen;
    /// the main track always plays. The other tracks' settings stay as they are, so un-soloing restores them
    /// (`Solo`). Overlay and audio tracks only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
    /// Sorted by start; never overlapping.
    pub items: Vec<Item>,
}

impl Track {
    pub fn new(kind: TrackKind, name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            kind,
            name: name.into(),
            hidden: false,
            muted: false,
            locked: false,
            solo: false,
            items: Vec::new(),
        }
    }
}

/// What the tracks' solo buttons leave on (non-destructive: each track's own hide and mute are untouched). Resolved
/// once per project revision, O(tracks); asking it about a track is O(1). The main track isn't a `Track`: it always
/// plays and shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Solo {
    /// Some overlay or audio track is soloed: only soloed tracks are heard.
    pub sound: bool,
    /// Some overlay track is soloed: only soloed overlay tracks are seen.
    pub picture: bool,
}

impl Solo {
    pub fn of(tracks: &[Track]) -> Self {
        let mut solo = Self::default();
        for track in tracks
            .iter()
            .filter(|t| t.solo && t.kind != TrackKind::Zoom)
        {
            solo.sound = true;
            solo.picture |= track.kind == TrackKind::Overlay;
        }
        solo
    }

    /// Whether `track`'s pictures are drawn: an overlay track that isn't hidden, and soloed while any overlay track is.
    pub fn shows(self, track: &Track) -> bool {
        track.kind == TrackKind::Overlay && !track.hidden && (track.solo || !self.picture)
    }

    /// Whether `track`'s sound is heard: not muted (nor a hidden overlay track), and soloed while any track is. Mute
    /// wins over solo.
    pub fn sounds(self, track: &Track) -> bool {
        let silenced = track.muted || (track.kind == TrackKind::Overlay && track.hidden);
        !silenced && (track.solo || !self.sound)
    }
}

/// Ties an item's start to a moment of main-track content (survives splits and moves).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Attach {
    /// Preferred clip; falls back to any clip with the same source covering `source_us`.
    pub clip_id: Id,
    pub source: ClipSource,
    pub source_us: Us,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum ZoomTarget {
    /// Planned by the smart camera from the recorded cursor; `using` empty means every kind.
    Auto {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        using: Vec<FocusKind>,
    },
    Rect {
        rect: BoxN,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Outline {
    pub color: String,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextStyle {
    /// The family and where it comes from (`klipolo_fonts::resolve` picks the face of `weight` and `italic`).
    pub font: FontRef,
    /// Pixels at a 1080-pixel short side.
    pub size: f64,
    pub weight: u16,
    #[serde(default)]
    pub italic: bool,
    pub color: String,
    pub background: Option<String>,
    pub outline: Option<Outline>,
    pub shadow: bool,
    pub align: TextAlign,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font: FontRef::inter(),
            size: 64.0,
            weight: 700,
            italic: false,
            color: "#FFFFFF".into(),
            background: None,
            outline: None,
            shadow: true,
            align: TextAlign::Center,
        }
    }
}

/// A layer that is a recording source's own sound or picture: which recording and source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Tie {
    pub recording: Id,
    pub source: Id,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ItemContent {
    Video {
        asset_id: Id,
        source_in_us: Us,
        speed: f64,
        volume: f64,
        muted: bool,
        /// Shown as in a mirror (a separated camera keeps its bubble's). Only the picture flips as it is
        /// drawn: the file is its own. Left out of the JSON while off.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        mirror: bool,
        /// The recording source this layer is that source's own sound or picture of: set when
        /// `DetachSource` separates it, or when Sync lines it up; a nudge or a sync again of that source moves it.
        /// Splits and edge trims keep it; slipping its file time by hand drops it. Left out of the JSON while None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tie: Option<Tie>,
    },
    Image {
        asset_id: Id,
    },
    /// It comes in and goes out with `layout.motion` presets (`klipolo.builtin/motion.fade`, `…typewriter`).
    Text {
        text: String,
        style: TextStyle,
    },
    Audio {
        asset_id: Id,
        source_in_us: Us,
        volume: f64,
        /// Silenced by its own switch, keeping its volume, so unmuting brings that volume back; left out of the JSON
        /// while off.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        muted: bool,
        fade_in_us: Us,
        fade_out_us: Us,
        /// An audio effect contribution (`klipolo.builtin/audio.voice`), or none.
        effect: Option<ContribId>,
        /// The file's clock drift against the recording's (a separated recording source's run): it plays
        /// `1 + drift_ppm·10⁻⁶` of its own time per timeline µs. Left out of the JSON while 0.
        #[serde(default, skip_serializing_if = "is_zero")]
        drift_ppm: f64,
        /// Plays only this channel of the file, centred, as its source's mix did (`SourceMix::centre_from`: a
        /// separated one-sided mic); None plays the file as recorded. Left out of the JSON while None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        centre_from: Option<u8>,
        /// The recording source this layer is that source's own sound or picture of: set when
        /// `DetachSource` separates it, or when Sync lines it up; a nudge or a sync again of that source moves it.
        /// Splits and edge trims keep it; slipping its file time by hand drops it. Left out of the JSON while None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tie: Option<Tie>,
        /// Ducking: played this many dB lower (-30 to 0, `validate::DEEPEST_DUCK_DB`) while someone speaks, as music is
        /// under a voice, easing down just before the words and back up after them (the engine's mix). None plays it at
        /// its volume throughout. Left out of the JSON while None.
        ///
        /// It also says what the layer is (`media::music::is_music`): None is a sound effect; `Some(0.0)` is music with
        /// ducking off; below 0 is music ducking that deep. Music fits the video's end as it changes, so turning ducking
        /// off writes `Some(0.0)`, never None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        duck: Option<f64>,
    },
    Zoom {
        target: ZoomTarget,
        scale: f64,
    },
    /// A picture drawn by a contribution from its parameters: a shader generator (a gradient) or a scene (a motion
    /// graphic, `contrib::GENERATOR_SOURCES`). Built with `ItemContent::generator`.
    Generator {
        source: ContribId,
        #[serde(default)]
        params: BTreeMap<String, ParamValue>,
        /// Draws the settled pose, not moving (Remove motion on a motion graphic).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        still: bool,
        /// How far into the contribution's time this piece starts (`timeline::piece_time`); 0 unless split.
        #[serde(default, skip_serializing_if = "is_zero_us")]
        offset_us: Us,
        /// The whole layer's length the contribution's segments map onto, set by a split; None maps them onto this
        /// layer's own length.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        span_us: Option<Us>,
        /// The id of the layer this piece was split from, set by a split on both pieces and kept by later ones: every
        /// piece seeds its effects (`kl.seed`) as the whole layer did, so random patterns continue across a cut. None
        /// seeds from the layer's own id. Only ever hashed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        seed_from: Option<Id>,
    },
    /// No picture of its own: the layer's effects re-draw what is below it, inside its box.
    Adjustment,
}

impl ItemContent {
    /// The recording source a sound or video layer is tied to, if any.
    pub fn tie(&self) -> Option<&Tie> {
        match self {
            Self::Audio { tie, .. } | Self::Video { tie, .. } => tie.as_ref(),
            _ => None,
        }
    }

    /// Ties a sound or video layer to a recording source, or frees it; other layers have no tie.
    pub fn set_tie(&mut self, to: Option<Tie>) {
        if let Self::Audio { tie, .. } | Self::Video { tie, .. } = self {
            *tie = to;
        }
    }

    /// The file a sound or video layer plays and where it plays it from.
    pub fn played(&self) -> Option<(&Id, Us)> {
        match self {
            Self::Audio {
                asset_id,
                source_in_us,
                ..
            }
            | Self::Video {
                asset_id,
                source_in_us,
                ..
            } => Some((asset_id, *source_in_us)),
            _ => None,
        }
    }

    pub fn track_kind(&self) -> TrackKind {
        match self {
            Self::Video { .. }
            | Self::Image { .. }
            | Self::Text { .. }
            | Self::Generator { .. }
            | Self::Adjustment => TrackKind::Overlay,
            Self::Audio { .. } => TrackKind::Audio,
            Self::Zoom { .. } => TrackKind::Zoom,
        }
    }

    /// Layers that draw a picture of their own: video, image, text and generators (shaders and scenes). An adjustment
    /// box only changes what is below it, as sound and zooms draw nothing: none of those make the video longer
    /// (`query::video_duration`).
    ///
    /// Exhaustive on purpose: a new kind must say whether it sets the length (and the UI's `hasOwnPicture` follows).
    pub fn has_own_picture(&self) -> bool {
        match self {
            Self::Video { .. }
            | Self::Image { .. }
            | Self::Text { .. }
            | Self::Generator { .. } => true,
            Self::Audio { .. } | Self::Zoom { .. } | Self::Adjustment => false,
        }
    }

    /// Layers that draw (and so may carry effects).
    pub fn is_visual(&self) -> bool {
        matches!(
            self,
            Self::Video { .. }
                | Self::Image { .. }
                | Self::Text { .. }
                | Self::Generator { .. }
                | Self::Adjustment
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Item {
    pub id: Id,
    /// Timeline start. For attached items this is recomputed from `attach` after every edit.
    pub start_us: Us,
    pub duration_us: Us,
    pub attach: Option<Attach>,
    pub layout: Layout,
    pub content: ItemContent,
    /// The layer's effects, in order; only layers that draw have them.
    #[serde(default)]
    pub effects: Vec<EffectInstance>,
}

impl Item {
    pub fn end_us(&self) -> Us {
        self.start_us.saturating_add(self.duration_us)
    }
}
