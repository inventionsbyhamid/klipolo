//! The render worker's protocol: JSON lines over its stdin and stdout, one request at
//! a time, each request at most `MAX_REQUEST_BYTES` and each reply line at most `MAX_REPLY_LINE`. Replies carry bytes,
//! never paths: images and scenes go back base64-encoded (`Blob`), each image on its own line, and every reply
//! type refuses a field it doesn't have, so a reply that names a path is refused. Large inputs travel as files
//! the parent wrote into the session's `in/`, named by an `InputName`.

mod blob;
mod input;
mod specs;

use klipolo_model::contrib::{ContribDef, ContribId, Resolution};
use serde::{Deserialize, Serialize};

pub use super::budget::Cost;
pub use super::stats::{FrameStats, Luma};
use crate::registry::ShaderKind;
use crate::registry::guard::{Diagnostic, StaticFacts};
pub use blob::{Blob, Key, MAX_FRAME_EDGE, THUMBNAIL_EDGE, fit_jpeg, reencode_jpeg};
pub use input::{Input, InputName, PROFILE_NAME};
pub use specs::{
    DraftSpec, FrameSource, FramesSpec, MIN_FRAME_EDGE, MeasureSpec, SizeName, SourceRef,
};

/// A blob's bytes, decoded.
pub const MAX_BLOB_BYTES: usize = 600 * 1024;
pub const MAX_REPLY_LINE: usize = 1 << 20;
pub const MAX_REQUEST_BYTES: usize = 1 << 20;
/// Frames one request draws (`previewFrames` at most 9 images).
pub const MAX_FRAMES: usize = 9;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum WorkerRequest {
    /// Checks a shader's text. No GPU.
    Validate {
        id: u64,
        kind: WireKind,
        def: ContribDef,
        body: String,
    },
    /// Checks, hardens, compiles and measures a draft; its thumbnail comes on a line of its own first.
    Compile {
        id: u64,
        kind: WireKind,
        def: ContribDef,
        body: String,
        measure: MeasureSpec,
    },
    Measure {
        id: u64,
        source: SourceRef,
        measure: MeasureSpec,
    },
    Frames {
        id: u64,
        spec: FramesSpec,
    },
    /// Checks a library or embedded contribution again on this computer: `folder` is the parent's copy of
    /// the bytes it loaded, `key` their key. A scene's carries `contribs`: the generators its fills
    /// name, as `CheckScene`'s.
    Verify {
        id: u64,
        contrib: ContribId,
        folder: InputName,
        key: Key,
        measure: MeasureSpec,
        #[serde(default)]
        contribs: Option<InputName>,
    },
    /// Checks and measures an agent's scene from files in `in/`: the scene's JSON, the SVGs its `svg` nodes
    /// name (each its name, a line feed and its text), and `contribs`: the library and project generators its
    /// fills name, with this computer's verdicts and envelopes (`frames::ContribEntry`).
    CheckScene {
        id: u64,
        def: ContribDef,
        scene: InputName,
        svgs: Vec<InputName>,
        contribs: InputName,
        measure: MeasureSpec,
        /// Checked and normalised only, with no GPU and no wait for idle (the app's answer while Klipolo is busy): no
        /// cost, no thumbnail. The app measures the normalised bytes once idle.
        #[serde(default)]
        check_only: bool,
    },
    Shutdown,
}

impl WorkerRequest {
    /// None for `Shutdown`.
    pub fn id(&self) -> Option<u64> {
        match self {
            Self::Validate { id, .. }
            | Self::Compile { id, .. }
            | Self::Measure { id, .. }
            | Self::Frames { id, .. }
            | Self::Verify { id, .. }
            | Self::CheckScene { id, .. } => Some(*id),
            Self::Shutdown => None,
        }
    }

    /// Whether it measures, so it waits until the preview isn't playing and no export runs: and so do frames of
    /// the project, which carry the person's video.
    pub fn waits_for_idle(&self) -> bool {
        match self {
            Self::Compile { .. } | Self::Measure { .. } | Self::Verify { .. } => true,
            Self::CheckScene { check_only, .. } => !check_only,
            Self::Frames { spec, .. } => spec.has_video(),
            Self::Validate { .. } | Self::Shutdown => false,
        }
    }

    /// The image lines its reply may bring: one thumbnail, or up to `MAX_FRAMES` frames. More is a bad reply, so a
    /// worker can't grow the app's memory until the deadline.
    pub fn max_images(&self) -> usize {
        match self {
            Self::Compile { .. } | Self::Verify { .. } | Self::CheckScene { .. } => 1,
            Self::Frames { .. } => MAX_FRAMES,
            Self::Validate { .. } | Self::Measure { .. } | Self::Shutdown => 0,
        }
    }

    /// Why the client refuses it before sending (the protocol's limits), if it does.
    pub fn refusal(&self) -> Option<&'static str> {
        match self {
            Self::Frames { spec, .. } => spec.refusal(),
            _ => None,
        }
    }

    /// The step a refusal before any work names.
    pub fn first_step(&self) -> Step {
        match self {
            Self::Validate { .. } | Self::Compile { .. } | Self::Verify { .. } | Self::Shutdown => {
                Step::Scan
            }
            Self::Measure { .. } => Step::Measure,
            Self::Frames { .. } => Step::Frames,
            Self::CheckScene { .. } => Step::Parse,
        }
    }

    /// Whether `reply` may answer it: its id, and a final reply of its kind (or `Failed`); images only where the
    /// request makes them.
    pub fn answered_by(&self, reply: &WorkerReply) -> bool {
        use WorkerReply as R;
        let Some(id) = self.id() else {
            return false;
        };
        match reply {
            R::Ready { .. } => false,
            R::Progress { id: r, .. } => *r == id,
            // A request the worker couldn't read has no id it can name.
            R::Failed { id: r, .. } => *r == id || *r == 0,
            R::Validated { id: r, .. } => *r == id && matches!(self, Self::Validate { .. }),
            R::Compiled { id: r, .. } => {
                *r == id
                    && matches!(
                        self,
                        Self::Compile { .. } | Self::Verify { .. } | Self::CheckScene { .. }
                    )
            }
            R::Thumbnail { id: r, .. } => {
                *r == id
                    && matches!(
                        self,
                        Self::Compile { .. } | Self::Verify { .. } | Self::CheckScene { .. }
                    )
            }
            R::Measured { id: r, .. } => {
                *r == id && matches!(self, Self::Measure { .. } | Self::Verify { .. })
            }
            R::Frame { id: r, .. } | R::Framed { id: r, .. } => {
                *r == id && matches!(self, Self::Frames { .. })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum WorkerReply {
    /// The first line, before any request: `sandboxed` false where this OS has no worker sandbox yet.
    Ready {
        version: String,
        sandboxed: bool,
    },
    Validated {
        id: u64,
        ok: bool,
        diagnostics: Vec<Diagnostic>,
        facts: Option<StaticFacts>,
    },
    Compiled {
        id: u64,
        ok: bool,
        diagnostics: Vec<Diagnostic>,
        facts: Option<StaticFacts>,
        cost: Option<Cost>,
        resolved: Option<Resolution>,
        /// The key of what it measured.
        key: Option<Key>,
        /// SHA-256 of the hardened text it compiled: the verified store notices when a naga update changes it under
        /// the same key.
        #[serde(default)]
        hardened: Option<Key>,
        /// The normalised scene bytes.
        scene: Option<Blob>,
    },
    /// On its own line.
    Thumbnail {
        id: u64,
        jpeg: Blob,
    },
    Measured {
        id: u64,
        cost: Cost,
        /// SHA-256 of the hardened text it measured; None for a scene.
        hardened: Option<Key>,
    },
    /// One line a frame.
    Frame {
        id: u64,
        index: u32,
        t: f64,
        jpeg: Blob,
        stats: FrameStats,
    },
    Framed {
        id: u64,
        count: u32,
        cost: Option<Cost>,
        /// What the frames said (`frame.fontMissing`, a contribution left out), each once, escaped.
        #[serde(default)]
        notes: Vec<String>,
    },
    Progress {
        id: u64,
        step: Step,
    },
    Failed {
        id: u64,
        step: Step,
        reason: String,
    },
}

impl WorkerReply {
    /// Whether it ends its request: progress, thumbnails and frames come before the one that does.
    pub fn is_final(&self) -> bool {
        !matches!(
            self,
            Self::Ready { .. }
                | Self::Progress { .. }
                | Self::Thumbnail { .. }
                | Self::Frame { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireKind {
    Filter,
    Generator,
    Transition,
    Scene,
}

impl WireKind {
    /// The shader kind; None for a scene (`CheckScene`).
    pub fn shader(self) -> Option<ShaderKind> {
        match self {
            Self::Filter => Some(ShaderKind::Filter),
            Self::Generator => Some(ShaderKind::Generator),
            Self::Transition => Some(ShaderKind::Transition),
            Self::Scene => None,
        }
    }
}

/// Where a request is when a reply or an error names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Scan,
    Parse,
    Rules,
    Harden,
    Compile,
    Measure,
    Frames,
    Svg,
}

impl Step {
    /// What the worker was doing, for the agent (English).
    pub fn doing(self) -> &'static str {
        match self {
            Self::Scan => "reading the source",
            Self::Parse => "parsing",
            Self::Rules => "checking the rules",
            Self::Harden => "adding the loop guards",
            Self::Compile => "compiling",
            Self::Measure => "measuring",
            Self::Frames => "drawing frames",
            Self::Svg => "importing the SVGs",
        }
    }
}
