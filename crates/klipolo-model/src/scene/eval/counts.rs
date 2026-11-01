//! What a frame draws, counted against the runtime caps; the renderer degrades what is past them, with one diagnostic
//! per scene.

use super::super::SceneLimits;

/// What a frame draws, counted (the runtime caps).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCounts {
    /// Drawn instance nodes (not repeats or particle shapes).
    pub instances: usize,
    /// Instance nodes the scene asks for past `MAX_INSTANCES`, never built.
    pub dropped_instances: usize,
    pub particles: usize,
    /// Live particles past `MAX_PARTICLES` (or past the work a frame may do), not written: at most this many.
    pub dropped_particles: usize,
    /// Split units drawn blurred: on a drawn text, above 0 opacity, with the node's or their own blur.
    pub blurring_units: usize,
    /// Characters of every drawn text.
    pub chars: usize,
    /// The most characters one drawn text shows.
    pub most_chars: usize,
    /// Groups drawn offscreen.
    pub isolated_groups: usize,
    /// Tracks read, every one a frame needs: a frame is never cut short. An instance that could read past
    /// `MAX_TRACKS` is decided once, when it is built, and every frame then copies its still (`still`, 0 reads;
    /// shown with one diagnostic).
    pub track_evals: usize,
    /// Particle candidates looked at, at most four per particle a frame may write.
    pub particle_looks: usize,
    /// The instance can read more than `MAX_TRACKS` tracks a frame: this is its settled still, the same every frame.
    pub still: bool,
}

/// Which runtime caps a frame is past.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapsOver {
    pub blurring_units: bool,
    pub node_chars: bool,
    pub scene_chars: bool,
    pub instances: bool,
    pub particles: bool,
    pub isolated_groups: bool,
    pub tracks: bool,
}

impl CapsOver {
    pub fn any(&self) -> bool {
        self.blurring_units
            || self.node_chars
            || self.scene_chars
            || self.instances
            || self.particles
            || self.isolated_groups
            || self.tracks
    }
}

impl FrameCounts {
    pub fn over(&self) -> CapsOver {
        CapsOver {
            blurring_units: self.blurring_units > SceneLimits::MAX_BLURRING_UNITS,
            node_chars: self.most_chars > SceneLimits::MAX_TEXT_NODE,
            scene_chars: self.chars > SceneLimits::MAX_TEXT_SCENE,
            instances: self.dropped_instances > 0,
            particles: self.dropped_particles > 0,
            isolated_groups: self.isolated_groups > SceneLimits::MAX_GROUPS_OFFSCREEN,
            tracks: self.still || self.track_evals > SceneLimits::MAX_TRACKS,
        }
    }
}
