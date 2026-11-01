use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contrib::ContribId;
use crate::cursor_style::CursorStyle;
use crate::font::FontRef;
use crate::ids::{new_id, Id};
use crate::time::Us;
use crate::timeline::{Clip, Timeline};

pub use crate::recording::{
    GameInfo, Marker, MarkerKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole,
    SourceRun,
};
pub use crate::synced::{SyncLevel, Synced, SyncedBy};

/// Format 7: definitions and generator layers gain optional fields (a generator piece's place in its
/// contribution's time), and settings hold fonts as `FontRef`s and tables as rows. Format 6: the captions
/// name a style, a look, a colour, a font and a place per format. Format 5: a text names its font and where
/// it comes from (`FontRef`). Format 4 made recordings sources with runs. Older files are refused, never migrated.
pub const PROJECT_VERSION: u32 = 7;

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    TS,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FormatKind {
    Landscape,
    Portrait,
    Square,
}

impl FormatKind {
    /// Canvas size in pixels at 1080p.
    pub fn size(self) -> (u32, u32) {
        match self {
            Self::Landscape => (1920, 1080),
            Self::Portrait => (1080, 1920),
            Self::Square => (1080, 1080),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Settings {
    pub fps: u32,
    pub formats: Vec<FormatKind>,
    pub primary_format: FormatKind,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fps: 30,
            formats: vec![FormatKind::Landscape],
            primary_format: FormatKind::Landscape,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AssetKind {
    Video,
    Audio,
    Image,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Asset {
    pub id: Id,
    pub kind: AssetKind,
    pub path: String,
    pub name: String,
    pub duration_us: Us,
    pub width: u32,
    pub height: u32,
    /// 0 for audio and images.
    pub fps: f64,
    /// The clockwise turn (0, 90, 180 or 270) a video's display matrix asks for, probed at import so opening its
    /// decoder never probes the file again; None when not probed (the decoder probes it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rotation: Option<u16>,
    pub has_audio: bool,
    /// The file's sound starts this long after its picture (the probe's stream start times, each stream counted from
    /// its own start; negative when the sound starts first). Its file time is its picture's, so its
    /// sound plays at file time minus this wherever it plays (`audio_plan`), and its transcript and levels, which are
    /// in its sound's time, map back by adding it (`speech::voice_runs`). Read through `lead_us`, the one value every
    /// reader takes: playback, transcripts, waveforms, and Sync with its hand marks.
    ///
    /// None means never probed: a video imported before the lead was read, filled from the probe when its project
    /// opens; sound files and images stay None (no lead). Left out of the JSON while None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub audio_lead_us: Option<Us>,
    /// The tool that made the file, when Klipolo had one make it ("Muse Image" for a sticker or a title card made in
    /// chat): the library groups such files under its name. None for a file the person brought; left out of the JSON
    /// while None. A build from before this field reads the project but drops it on its next save, so the library then
    /// falls back to the file's `<stem> Media/Muse Image/` folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub made_with: Option<String>,
}

/// The furthest a file's sound may start from its picture: past it the probe takes a broken stream start, not a lead.
pub const MAX_AUDIO_LEAD_US: Us = 500_000;

impl Asset {
    /// `audio_lead_us` within ±`MAX_AUDIO_LEAD_US`, as every reader of a file's sound takes it: a hand-edited project
    /// can't push a time out of range.
    pub fn lead_us(&self) -> Us {
        self.audio_lead_us
            .unwrap_or(0)
            .clamp(-MAX_AUDIO_LEAD_US, MAX_AUDIO_LEAD_US)
    }
}

/// A custom colour LUT. The project only references its `.cube` file; the engine loads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ColorLut {
    pub id: Id,
    pub name: String,
    /// The `.cube` file.
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BackgroundKind {
    None,
    Solid,
    Gradient,
    Image,
    /// The main footage itself, covering the frame behind it, blurred and dimmed: where a clip doesn't fill the frame
    /// (Fit, or the padding) it shows its own picture around it rather than bars.
    Blur,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Background {
    pub kind: BackgroundKind,
    pub color: String,
    pub color_end: Option<String>,
    pub asset_id: Option<Id>,
    /// Inset of the main footage, as a fraction of the shorter canvas side (0 to 0.4).
    pub padding: f64,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            kind: BackgroundKind::None,
            color: "#101114".into(),
            color_end: None,
            asset_id: None,
            padding: 0.0,
        }
    }
}

/// The project's captions: on or off, in a style (`captions::styles`), with the person's look, colour,
/// font and place per format over it. Everything but the switch is left out of the JSON until set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CaptionSettings {
    pub enabled: bool,
    /// None until captions are first turned on, which writes the formats' default (`default_style`); from then on
    /// the project's own, whatever formats it gains. An id this build doesn't have is kept and drawn in Clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub style: Option<ContribId>,
    /// One of the style's looks (`VariantDef::key`); None is the style's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub variant: Option<String>,
    /// `#RRGGBB` over the style's accent; None is the style's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub accent: Option<String>,
    /// The person's font over the style's; None is the style's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub font: Option<FontRef>,
    /// The block's centre y per format, as a share of the frame's height (x is centred); a format without one takes the
    /// style's `position`. Only listed formats keep theirs.
    #[serde(default)]
    pub place: BTreeMap<FormatKind, f64>,
    /// 9:16: the style's place also keeps clear of the platforms' ad buttons.
    #[serde(default)]
    pub ad_safe: bool,
    /// The captions' BCP 47 tag, for the Chinese, Japanese and Korean faces; None follows the transcripts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Loudness {
    /// The mix is left exactly as it is: no gain in the preview or any export.
    Off,
    /// −14 LUFS integrated, where YouTube, Shorts, Reels and TikTok normalise: the default, so a creator who never
    /// touches the setting gets platform-ready sound.
    Youtube,
    /// −16 LUFS integrated.
    Podcast,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioSettings {
    pub loudness: Loudness,
}

impl Default for AudioSettings {
    /// New projects aim at the platforms' −14 LUFS (`SetAudio` changes it, chat included).
    fn default() -> Self {
        Self {
            loudness: Loudness::Youtube,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Project {
    pub version: u32,
    pub id: Id,
    pub name: String,
    pub settings: Settings,
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub recordings: Vec<Recording>,
    #[serde(default)]
    pub luts: Vec<ColorLut>,
    pub timeline: Timeline,
    #[serde(default)]
    pub captions: CaptionSettings,
    #[serde(default)]
    pub background: Background,
    #[serde(default)]
    pub audio: AudioSettings,
    /// How the cursor is drawn; a clip may have its own (`Clip::cursor`).
    #[serde(default)]
    pub cursor: CursorStyle,
    /// The person's corrections of the words heard, sorted by asset and start, never overlapping on one
    /// file. Captions and the Script view read through them; the recordings never change.
    // Shared (copy on write): only an op that changes the list copies it, so every other apply, dry run and diff
    // costs O(1) for it.
    #[serde(default, skip_serializing_if = "no_items")]
    pub word_fixes: Arc<Vec<crate::speech::WordFix>>,
    /// The person's look for caption words, in each file's own time like the fixes: sorted by asset
    /// and start, never overlapping on one file. A style switch keeps them.
    // Shared as the fixes are.
    #[serde(default, skip_serializing_if = "no_items")]
    pub word_looks: Arc<Vec<crate::captions::looks::WordLook>>,
    /// Features beyond this build's that the file needs (`contrib::FEATURES`). A build that doesn't know one opens
    /// the project read-only rather than drop data on save.
    #[serde(default)]
    pub requires: BTreeSet<String>,
}

/// A shared list is left out of the file when it's empty.
fn no_items<T>(list: &Arc<Vec<T>>) -> bool {
    list.is_empty()
}

// The shared lists' items are `Eq`, so `Arc`'s `==` compares pointers first and a diff of an untouched list is O(1): a
// build that drops `Eq` from either fails here, not quietly back to comparing item by item.
const _: fn() = || {
    fn eq<T: Eq>() {}
    eq::<crate::speech::WordFix>();
    eq::<crate::captions::looks::WordLook>();
};

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: PROJECT_VERSION,
            id: new_id(),
            name: name.into(),
            settings: Settings::default(),
            assets: Vec::new(),
            recordings: Vec::new(),
            luts: Vec::new(),
            timeline: Timeline::default(),
            captions: CaptionSettings::default(),
            background: Background::default(),
            audio: AudioSettings::default(),
            cursor: CursorStyle::default(),
            word_fixes: Arc::default(),
            word_looks: Arc::default(),
            requires: BTreeSet::new(),
        }
    }

    pub fn asset(&self, id: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    pub fn recording(&self, id: &str) -> Option<&Recording> {
        self.recordings.iter().find(|r| r.id == id)
    }

    /// The cursor style `clip` draws with: its own, else the project's.
    pub fn cursor_for<'a>(&'a self, clip: &'a Clip) -> &'a CursorStyle {
        clip.cursor.as_ref().unwrap_or(&self.cursor)
    }
}
