//! The caption styles this build draws: the built-in files, and later packs' (`with`).
//!
//! Adding a built-in style is one data file in `captions/styles/` and one line in `builtin_styles!` below: that line
//! embeds the file, lists it in `BUILTIN_FILES` and gives the style its word in `BuiltinCaptionStyle`, which chat's
//! `setCaptions` takes and the UI's TypeScript keys its per-style records by (so the compiler asks for each). A test
//! fails while the folder and the list disagree.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::style::CaptionStyleDef;
use crate::coded::CodedError;
use crate::contrib::{ContribDef, ContribId, ContribKind};
use crate::messages::code;
use crate::project::{CaptionSettings, FormatKind};

macro_rules! builtin_styles {
    ($($(#[doc = $doc:literal])* $style:ident = $key:literal,)*) => {
        /// A built-in caption style, by its word (`bold`): the style is `klipolo.builtin/caption.<word>`, drawn from
        /// `captions/styles/<word>.json`.
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS, schemars::JsonSchema,
        )]
        #[ts(export)]
        pub enum BuiltinCaptionStyle {
            $($(#[doc = $doc])* #[serde(rename = $key)] $style,)*
        }

        impl BuiltinCaptionStyle {
            pub const ALL: &'static [BuiltinCaptionStyle] = &[$(BuiltinCaptionStyle::$style,)*];

            /// Its word, as JSON says it and as its file is named.
            pub fn key(self) -> &'static str {
                match self {
                    $(BuiltinCaptionStyle::$style => $key,)*
                }
            }
        }

        /// One line per built-in style: its word and its file. `tests/caption_styles.rs` checks this list against the
        /// folder.
        pub const BUILTIN_FILES: &[(&str, &str)] = &[
            $(($key, include_str!(concat!("../../captions/styles/", $key, ".json"))),)*
        ];
    };
}

builtin_styles! {
    /// Big uppercase words that pop in as they're said, one keyword in colour.
    Bold = "bold",
    /// One or two comic words with a thick black outline.
    Loud = "loud",
    /// Sentence case on rounded boxes that hug each line.
    Boxed = "boxed",
    /// A line where the spoken word lights up.
    Karaoke = "karaoke",
    /// A coloured box that follows each word.
    Highlight = "highlight",
    /// Calm subtitles for long videos and tutorials.
    Clean = "clean",
    /// Condensed glowing capitals for gameplay.
    Arcade = "arcade",
    /// Big uppercase words that pop in one by one as they're said, the word being said in colour.
    WordPop = "word-pop",
    /// Heavy uppercase words on a rounded box that hugs each line.
    BoldBox = "bold-box",
}

impl BuiltinCaptionStyle {
    /// `klipolo.builtin/caption.<word>`.
    pub fn id(self) -> ContribId {
        ContribId::builtin(&format!("caption.{}", self.key()))
    }
}

pub const DEFAULT_STYLE: &str = "klipolo.builtin/caption.bold";
/// What an unknown style draws in: captions never silently disappear. Also landscape-only's default.
pub const FALLBACK_STYLE: &str = "klipolo.builtin/caption.clean";
/// The schema version every style file names (`"schema": 1`); pack manifests will carry style files under the
/// `abi` major for this kind, so a newer schema is refused with its reason, never half-read.
pub const STYLE_SCHEMA: u32 = 1;

/// The style a project's captions take until the person picks one: Bold when any format is 9:16 or square,
/// Clean for landscape-only.
pub fn default_style(formats: &[FormatKind]) -> &'static str {
    if formats
        .iter()
        .any(|f| matches!(f, FormatKind::Portrait | FormatKind::Square))
    {
        DEFAULT_STYLE
    } else {
        FALLBACK_STYLE
    }
}

/// Only the schema of a style file, read first so a newer file is never judged by this build's fields.
#[derive(Deserialize)]
struct SchemaProbe {
    schema: u32,
}

impl CaptionStyleDef {
    /// A style file as this build reads it (built-ins now, packs later): its schema first, so a newer one says it
    /// needs a newer Klipolo whatever fields it adds; then the strict parse and `check`, whose reason names the
    /// field.
    pub fn from_json(json: &str) -> Result<CaptionStyleDef, CodedError> {
        let unreadable = |reason: String| {
            CodedError::new(code!("model.captions.styleUnreadable")).with("reason", reason)
        };
        let probe: SchemaProbe =
            serde_json::from_str(json).map_err(|e| unreadable(e.to_string()))?;
        if probe.schema > STYLE_SCHEMA {
            return Err(CodedError::new(code!("model.captions.styleNeedsNewer")));
        }
        let def: CaptionStyleDef =
            serde_json::from_str(json).map_err(|e| unreadable(e.to_string()))?;
        def.check().map_err(unreadable)?;
        Ok(def)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CaptionStyles {
    defs: BTreeMap<ContribId, Arc<CaptionStyleDef>>,
}

impl CaptionStyles {
    pub fn builtin() -> &'static CaptionStyles {
        static BUILTIN: OnceLock<CaptionStyles> = OnceLock::new();
        BUILTIN.get_or_init(|| {
            let mut s = CaptionStyles::default();
            for (key, json) in BUILTIN_FILES {
                let def = CaptionStyleDef::from_json(json).unwrap_or_else(|e| {
                    let reason = crate::coded::english(&[crate::messages::MESSAGES], &e);
                    panic!("{key}.json: {reason}")
                });
                s = s.with(def).unwrap_or_else(|e| panic!("{key}.json: {e}"));
            }
            s
        })
    }

    pub fn with(mut self, def: CaptionStyleDef) -> Result<Self, String> {
        def.check()?;
        self.defs.insert(def.id.clone(), Arc::new(def));
        Ok(self)
    }

    pub fn get(&self, id: &ContribId) -> Option<&Arc<CaptionStyleDef>> {
        self.defs.get(id)
    }

    pub fn defs(&self) -> impl Iterator<Item = &Arc<CaptionStyleDef>> {
        self.defs.values()
    }

    /// The style `s` draws in for a project with `formats`, with its variant and accent applied; `true` when its id
    /// isn't installed and Clean stands in (said as `frame.captionStyleMissing`). `style: None` exists only before the
    /// first enable (which writes `default_style`), so its previews follow the formats. Allocates one resolved copy:
    /// callers keep it while `s` is unchanged.
    pub fn for_settings(
        &self,
        s: &CaptionSettings,
        formats: &[FormatKind],
    ) -> (Arc<CaptionStyleDef>, bool) {
        let id = s
            .style
            .clone()
            .unwrap_or_else(|| ContribId::from(default_style(formats)));
        let (base, missing) = match self.get(&id) {
            Some(d) => (d, false),
            None => (
                self.get(&ContribId::from(FALLBACK_STYLE))
                    .expect("Clean is built in"),
                true,
            ),
        };
        let variant = s.variant.as_deref().filter(|_| !missing);
        (Arc::new(base.with(variant, s.accent.as_deref())), missing)
    }
}

/// The catalogue's entry for each built-in style (`ContribKind::CaptionStyle`): its id and name, no settings.
pub(crate) fn catalog_defs() -> impl Iterator<Item = ContribDef> {
    CaptionStyles::builtin()
        .defs()
        .map(|d| ContribDef::new(d.id.clone(), ContribKind::CaptionStyle, &d.name))
}
