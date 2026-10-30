//! The room a scene instance's frames are packed into: made when the instance
//! is built (and its text's when its text is), so packing a frame allocates nothing.

use klipolo_model::scene::instance::SceneInstance;

use super::backdrop::MAX_BACKDROPS;
use super::fills::MAX_FILLS;
use super::groups::{Iso, MAX_ISOS};
use super::meshes::Meshes;
use super::pack::{Packed, Run};
use super::path_pipeline::PathDraw;
use super::place::World;
use super::shapes::ShapeInstance;
use super::statics::NodeStatic;
use super::text_pipeline::{TextDraw, TextPaintDraw};

/// The room a frame of an instance takes, made when the instance is built.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Room {
    pub nodes: usize,
    /// Nodes drawn as paths (a fill and a stroke each, each a run between runs of shapes).
    pub paths: usize,
    /// Particles alive at once, and whether a particle shape is a path (each its own draws and runs).
    pub particles: usize,
    pub path_particles: bool,
    /// A group has a mask: its node may be drawn again in the mask's segment.
    pub masks: bool,
}

impl Room {
    /// The room a frame of `inst` (its statics, its meshes) takes.
    pub(super) fn of(inst: &SceneInstance, statics: &[NodeStatic], meshes: &Meshes) -> Self {
        Self {
            nodes: inst.len(),
            paths: meshes.count(),
            particles: inst.particle_capacity(),
            path_particles: statics
                .iter()
                .any(|s| s.particle && s.look == super::statics::Look::Path),
            masks: statics.iter().any(|s| s.mask.is_some()),
        }
    }
}

impl Packed {
    /// Room for a frame of an instance (`room`).
    pub(super) fn with_room(room: Room) -> Self {
        let again = if room.masks { 2 } else { 1 };
        let particle_paths = if room.path_particles {
            room.particles
        } else {
            0
        };
        let runs_room =
            (4 + 3 * (room.paths + particle_paths)) * again + 2 * MAX_ISOS + 4 * MAX_ISOS;
        Self {
            world: Vec::with_capacity(room.nodes),
            rest_world: Vec::with_capacity(room.nodes),
            shapes: Vec::with_capacity(2 * (room.nodes * again + room.particles)),
            runs: Vec::with_capacity(runs_room),
            draws: Vec::with_capacity(2 * (room.paths * again + particle_paths)),
            texts: Vec::new(),
            isos: Vec::with_capacity(MAX_ISOS),
            open: Vec::with_capacity(MAX_ISOS),
            fill_uses: Vec::with_capacity(MAX_FILLS),
            backdrop_uses: Vec::with_capacity(MAX_BACKDROPS),
            runs_room,
            ..Self::default()
        }
    }

    /// Room for an instance's text: `draws` text draws, the `paints` they name, and a run before and after each of
    /// `nodes` text nodes (made when its text is built: a later frame finds the room there).
    pub(super) fn reserve_text(&mut self, (draws, paints): (usize, usize), nodes: usize) {
        // Emptied first (`pack` empties them anyway), so room that is there is never asked for again.
        // Exactly that room: the GPU's buffers hold as many, and a draw never names a paint past them.
        self.texts.clear();
        self.texts.reserve_exact(draws);
        self.text_paints.clear();
        self.text_paints.reserve_exact(paints);
        self.runs.clear();
        self.runs.reserve(self.runs_room + 2 * nodes);
    }

    /// The CPU bytes the room takes.
    pub(super) fn bytes(&self) -> usize {
        use std::mem::size_of;
        (self.world.capacity() + self.rest_world.capacity()) * size_of::<World>()
            + self.shapes.capacity() * size_of::<ShapeInstance>()
            + self.runs.capacity() * size_of::<Run>()
            + self.draws.capacity() * size_of::<PathDraw>()
            + self.texts.capacity() * size_of::<TextDraw>()
            + self.text_paints.capacity() * size_of::<TextPaintDraw>()
            + self.isos.capacity() * size_of::<Iso>()
    }
}
