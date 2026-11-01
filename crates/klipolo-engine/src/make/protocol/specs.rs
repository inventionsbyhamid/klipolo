//! What a request names beside its id: what to measure or draw, at which sizes and settings. Every type
//! refuses a field it doesn't have.

use std::collections::BTreeMap;

use klipolo_model::contrib::ContribDef;
use klipolo_model::params::ParamValue;
use klipolo_model::project::FormatKind;
use serde::{Deserialize, Serialize};

use super::InputName;

/// What to measure: a draft, or a contribution folder the parent wrote into `in/`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum SourceRef {
    Draft { def: Box<ContribDef>, body: String },
    Input { name: InputName },
}

/// The sizes a measurement reports beside the preview size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SizeName {
    #[serde(rename = "preview")]
    Preview,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "2160p")]
    P2160,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeasureSpec {
    #[serde(default)]
    pub sizes: Vec<SizeName>,
    /// Frames each combination gets at preview size: 30 by default, at most 60.
    #[serde(default)]
    pub frames: u32,
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
    /// The measurement's time budget in ms, from the request's arrival: 0 is the
    /// server's (60 % of the request's deadline: 6 s for a compile, 9 s for a Measure), and more is cut to it. Past it,
    /// what isn't measured counts as Over.
    #[serde(default)]
    pub budget_ms: u32,
}

/// Frames to draw: each comes back on its own line as a JPEG with its statistics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FramesSpec {
    pub source: FrameSource,
    /// Seconds (at most `MAX_FRAMES`).
    pub times: Vec<f64>,
    #[serde(default = "landscape")]
    pub format: FormatKind,
    /// The frames' longest edge (at most `MAX_FRAME_EDGE`).
    pub long_edge: u32,
    /// The frames as one contact sheet, one image.
    #[serde(default)]
    pub sheet: bool,
    /// The client says the GPU is busy (the preview plays or an export runs): frames go at least 33 ms apart.
    #[serde(default)]
    pub busy: bool,
}

fn landscape() -> FormatKind {
    FormatKind::Landscape
}

/// A frame's longest edge, at least (`longEdge` 256–1024).
pub const MIN_FRAME_EDGE: u32 = 256;

impl FramesSpec {
    /// Frames of the project: the person's video in context (the longer deadline, and the request waits for idle).
    pub fn has_video(&self) -> bool {
        matches!(self.source, FrameSource::Project { .. })
    }

    /// Why the protocol doesn't allow these frames, if it doesn't: 1 to 9 finite times of 0 or more, a long edge of 256
    /// to 1024 px. The client refuses them before sending, and the worker does too.
    pub fn refusal(&self) -> Option<&'static str> {
        if self.times.is_empty()
            || self.times.len() > super::MAX_FRAMES
            || self.times.iter().any(|t| !t.is_finite() || *t < 0.0)
        {
            Some("frames take 1 to 9 times, each a number of seconds from 0")
        } else if !(MIN_FRAME_EDGE..=super::MAX_FRAME_EDGE).contains(&self.long_edge) {
            Some("a frame's long edge is 256 to 1024 px")
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum FrameSource {
    /// A draft or contribution drawn alone, over a mid-grey checkerboard so transparency shows; a scene's with the
    /// generators its fills name (`contribs`, as `CheckScene`'s).
    Alone {
        draft: Box<DraftSpec>,
        #[serde(default)]
        contribs: Option<InputName>,
    },
    /// A dry-run copy of the project: its snapshot, its non-built-in contributions (definition, source,
    /// verified `runsAt` and envelope; `make::frames::ContribEntry`) and a folder of the stills the app decoded, each a
    /// name in `in/`.
    Project {
        snapshot: InputName,
        contribs: InputName,
        stills: InputName,
    },
}

/// A shader as an agent wrote it, with the settings to draw it at (its defaults under them). The worker always checks
/// and hardens it itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftSpec {
    pub def: ContribDef,
    pub body: String,
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
}
