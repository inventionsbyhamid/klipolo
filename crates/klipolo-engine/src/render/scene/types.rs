//! The scene drawer's small shared types: what it did, where a layer is drawn,
//! where a frame's nodes land.

/// What the drawer did, for tests and the performance numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneStats {
    /// Instances built, ahead (`prepare`) or on a frame.
    pub builds: u64,
    /// Instances a frame had to build itself (the lookahead didn't prepare them).
    pub draw_builds: u64,
    /// The CPU time the last scene drawn took here (evaluate, pack, upload, encode), in microseconds.
    pub cpu_us_last: f64,
    /// Frames drawn from a held target, with no pass.
    pub held_frames: u64,
    /// Instances kept now.
    pub cached: usize,
    /// Path meshes kept, the bytes they hold, and how many were ever made (a trim, a dash or a motion makes
    /// none).
    pub meshes: usize,
    pub mesh_bytes: u64,
    pub meshes_made: u64,
    /// The bytes the scene pass's stencils hold (under their own bound, and in the scene bytes peak).
    pub stencil_bytes: u64,
    /// Scene texts kept (one per instance and size), and the bytes their atlases and draws hold.
    pub texts: usize,
    pub text_bytes: u64,
    /// Text builds asked and not landed yet, and how many of them wait for this computer's fonts (an export's;
    /// a frame waits for its build apart from that).
    pub text_pending: usize,
    pub text_pending_waiting: usize,
    /// The last scene drawn: its nodes drawn offscreen, and its bytes.
    pub offscreen: u32,
    pub bytes: SceneBytes,
    /// The pixels the last scene's own passes drew past its scene pass: its backdrops' blurs, its offscreen nodes'
    /// effects and lay-backs (the performance numbers).
    pub pass_pixels: u64,
    /// The texture fetches the last scene's letter blurs made in their own passes (the text pass reads each
    /// blurring fragment once more).
    pub blur_fetches: u64,
    /// How long the last survey of an instance's timeline took (µs), and how many ran (the
    /// the first frame of a moving scene, or the frame its own text lands).
    pub survey_us: f64,
    pub surveys: u64,
    /// The text build thread's time for the last survey a frame adopted from it (off the frame).
    pub survey_built_us: f64,
    /// The bytes the last scene's text draws and paints wrote to the GPU (only what changed).
    pub text_written: u64,
    /// The render thread's time at the start of the last frame a text build landed on (its atlas comes
    /// uploaded from the build thread).
    pub land_us: f64,
}

/// What a scene layer's frame drew: the target holding it, and where that target sits in the layer's (x, y, w, h,
/// pixels): the whole of it, or only what the scene drew (the compositor places it as that part of its quad).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneOut {
    pub target: usize,
    pub rect: [u32; 4],
}

/// What one scene instance holds while drawn, from its last
/// frame:
/// - `own`: its own target, only as big as what it draws (the layer's whole target when its effects, its
///   corners or a backdrop read it), counted with its layer as a layer's target is, so not in [`SceneBytes::total`];
/// - `targets`: the bytes of the pooled targets its passes held beyond its own target and the lay's ping-pong partner
///   (offscreen nodes and their effects' passes, each at its own box's size; a mask; a backdrop's grab and blurred
///   copies), counted by the pool itself size by size (the most of each size busy at once: what a fresh pool allocates
///   for the frame and a warm one keeps; `EffectStage::watch`);
/// - `buffers`: CPU and GPU, the instance, its frame and packing room, shapes, path draws, meshes, its targets' slots
///   and its passes' stencils (one a target size);
/// - `atlas`: its text's atlas, draws and CPU layouts;
/// - `scratch`: its targets of other sizes, its letters' blur scratch and its generator fills, at their own bytes;
/// - `partner`: the lay's ping-pong partner when a node was laid back into the scene's own target by a host pass (a
///   blend or a tilt), a target of the scene's own target's size that is the scene's own (as a filter's two ping-pong
///   targets are its own); a node laid back as a quad has none.
///
/// Reported to the render worker's measurement and counted against the pool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneBytes {
    pub own: u64,
    pub targets: u64,
    pub buffers: u64,
    pub atlas: u64,
    pub scratch: u64,
    pub partner: u64,
}

/// The scene's own share of its budget class's memory: its layer target (and its ping-pong partner), and its
/// buffers and atlas up to [`SCENE_OWN_BYTES`].
pub const SCENE_OWN_BYTES: u64 = 4 << 20;

/// A scene's bytes split for its budget class: its own, the extra layer-sized targets, and the extra bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneSplit {
    pub own: u64,
    pub extra_targets: u32,
    pub extra_bytes: u64,
}

impl SceneBytes {
    /// Everything it holds beyond its own target.
    pub fn total(&self) -> u64 {
        self.targets + self.buffers + self.atlas + self.scratch + self.partner
    }

    /// Split for the budget classes, one way for every caller (the scene classifier too): its own share is its own
    /// target (`own`, at most `layer`, its layer target's bytes; `layer` when not counted), the lay's ping-pong partner
    /// and its buffers and atlas up to [`SCENE_OWN_BYTES`]; its further targets' bytes, at their own sizes,
    /// are extra targets, each layer's worth or part of one counted as one (rounded up: a scene with any offscreen node
    /// is at best Normal, never Light); buffers and atlas past 4 MiB, and its scratch and fills, are extra bytes.
    pub fn split(&self, layer: u64) -> SceneSplit {
        let small = self.buffers + self.atlas;
        let own = if self.own == 0 {
            layer
        } else {
            self.own.min(layer)
        };
        SceneSplit {
            own: own + self.partner + small.min(SCENE_OWN_BYTES),
            extra_targets: self.targets.div_ceil(layer.max(1)) as u32,
            extra_bytes: small.saturating_sub(SCENE_OWN_BYTES) + self.scratch,
        }
    }
}

/// Where a scene layer is drawn: on a `canvas`-sized frame, into a target of `size` (its drawn size, its motion's
/// reach included). The drawn scale (`kl.px`) isn't part of it: a layer's motion changes that every frame, and the
/// meshes follow it by buckets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneAt {
    pub canvas: (u32, u32),
    pub size: (u32, u32),
}

/// Where a frame's nodes land: a target of `size` pixels, `k` of them per 1080p pixel (`kl.px`). One value where a
/// size and a scale went side by side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Drawn {
    pub size: (u32, u32),
    pub k: f64,
}
