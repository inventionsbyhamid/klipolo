//! Path meshes, shared: each tessellated once per (geometry, fill rule, stroke cap and join, the width
//! a round end is made for, tolerance bucket) and kept on the GPU while a scene uses it or until room is needed. The
//! same path in two layers, or in every instance of a repeat, is one mesh.
//!
//! **Bounded** at [`MAX_MESH_BYTES`], GPU and CPU together: past it the least recently used mesh no instance holds
//! goes; a mesh that still doesn't fit isn't made (its node draws nothing, said once as `frame.sceneTooDetailed`, as
//! a mesh past [`MAX_MESH_VERTICES`] is). A refusal is remembered (a few bytes) so it isn't tessellated again.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use wgpu::util::DeviceExt;

use super::path_pipeline::PathVertex;
use crate::render::effects::made;

/// The most bytes path meshes hold at once, GPU and CPU.
pub const MAX_MESH_BYTES: u64 = 32 << 20;
/// The most vertices one node's mesh (its fill, fringe and stroke together) may have.
pub const MAX_MESH_VERTICES: usize = 20_000;
/// What a remembered refusal counts against the bound.
const REFUSAL_BYTES: u64 = 64;

/// What a mesh is made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct MeshKey {
    /// The commands' hash (`geometry::hash`).
    pub geometry: u64,
    /// The fill rule (0 nonzero, 1 even-odd), None for no fill.
    pub fill: Option<u8>,
    /// Cap, join (0 butt or miter, 1 round, 2 square or bevel) and the round ends' width bucket (log2), None for no
    /// stroke.
    pub stroke: Option<(u8, u8, i8)>,
    /// The flattening tolerance's bucket: log2 of mesh units.
    pub tolerance: i8,
}

/// A mesh tessellated on the CPU: its vertices and triangles, and which index ranges are the fill (with its fringe)
/// and the stroke; the path's length and whether it is closed.
#[derive(Debug, Default)]
pub(super) struct MeshData {
    pub vertices: Vec<PathVertex>,
    pub indices: Vec<u16>,
    pub fill: (u32, u32),
    pub stroke: (u32, u32),
    pub length: f32,
    pub closed: bool,
}

/// A mesh on the GPU, and the box its vertices span with the longest of their directions (what an offscreen node's box
/// takes of a path draw: `bounds.rs`).
pub(super) struct GpuMesh {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub fill: (u32, u32),
    pub stroke: (u32, u32),
    pub length: f32,
    pub closed: bool,
    pub bounds: [f32; 5],
}

/// The box `vertices` span (x0, y0, x1, y1) and their longest direction.
fn bounds_of(vertices: &[PathVertex]) -> [f32; 5] {
    vertices.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
            0.0,
        ],
        |b, v| {
            let dir = (v.dir[0] * v.dir[0] + v.dir[1] * v.dir[1]).sqrt();
            [
                b[0].min(v.pos[0]),
                b[1].min(v.pos[1]),
                b[2].max(v.pos[0]),
                b[3].max(v.pos[1]),
                b[4].max(dir),
            ]
        },
    )
}

/// Why a mesh isn't drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refused {
    /// Past [`MAX_MESH_VERTICES`], or past the bound with everything else in use.
    TooDetailed,
}

struct Cached {
    mesh: Option<Arc<GpuMesh>>,
    bytes: u64,
    used: u64,
}

/// The shared meshes.
#[derive(Default)]
pub(super) struct MeshCache {
    meshes: HashMap<MeshKey, Cached>,
    bytes: u64,
    /// Meshes tessellated since the drawer was made.
    made: u64,
}

/// The most bytes path meshes have held at once in this process (`mesh_bytes_peak`).
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The most bytes path meshes have held at once in this process, GPU and CPU (a diagnostic).
pub fn mesh_bytes_peak() -> u64 {
    PEAK.load(Ordering::Relaxed)
}

impl MeshCache {
    /// The mesh for `key`, made by `make` (on the CPU) and uploaded when it isn't kept; `now` marks it used.
    pub(super) fn get(
        &mut self,
        device: &wgpu::Device,
        key: MeshKey,
        now: u64,
        make: impl FnOnce() -> Option<MeshData>,
    ) -> Result<Arc<GpuMesh>, Refused> {
        if let Some(c) = self.meshes.get_mut(&key) {
            c.used = now;
            return c.mesh.clone().ok_or(Refused::TooDetailed);
        }
        self.made += 1;
        let tessellated =
            make().filter(|m| m.vertices.len() <= MAX_MESH_VERTICES && !m.indices.is_empty());
        let Some(data) = tessellated else {
            self.refuse(key, now);
            return Err(Refused::TooDetailed);
        };
        let bytes = (std::mem::size_of_val(&data.vertices[..])
            + std::mem::size_of_val(&data.indices[..])) as u64
            + std::mem::size_of::<GpuMesh>() as u64;
        if !self.make_room(bytes) {
            return Err(Refused::TooDetailed);
        }
        let bounds = bounds_of(&data.vertices);
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Klipolo scene path"),
            contents: bytemuck::cast_slice(&data.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        // A copy's size is a multiple of 4: an odd count of 16-bit indices gets one more (unused).
        let mut indices = data.indices;
        if indices.len() % 2 == 1 {
            indices.push(0);
        }
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Klipolo scene path"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        made();
        made();
        let mesh = Arc::new(GpuMesh {
            vertices,
            indices: index_buffer,
            fill: data.fill,
            stroke: data.stroke,
            length: data.length,
            closed: data.closed,
            bounds,
        });
        self.remember(key, Some(mesh.clone()), bytes, now);
        Ok(mesh)
    }

    /// A refusal remembered (so the path isn't tessellated again) when there is room for it within the bound (a
    /// refusal counts too).
    fn refuse(&mut self, key: MeshKey, now: u64) {
        if self.make_room(REFUSAL_BYTES) {
            self.remember(key, None, REFUSAL_BYTES, now);
        }
    }

    fn remember(&mut self, key: MeshKey, mesh: Option<Arc<GpuMesh>>, bytes: u64, used: u64) {
        self.bytes += bytes;
        self.meshes.insert(key, Cached { mesh, bytes, used });
        PEAK.fetch_max(self.bytes, Ordering::Relaxed);
    }

    /// Room for `bytes` more: the least recently used meshes no instance holds (and refusals) go first. False when
    /// that isn't enough.
    fn make_room(&mut self, bytes: u64) -> bool {
        while self.bytes + bytes > MAX_MESH_BYTES {
            let free = self
                .meshes
                .iter()
                .filter(|(_, c)| c.mesh.as_ref().is_none_or(|m| Arc::strong_count(m) == 1))
                .min_by_key(|(_, c)| c.used)
                .map(|(k, _)| *k);
            let Some(key) = free else {
                return false;
            };
            if let Some(gone) = self.meshes.remove(&key) {
                self.bytes -= gone.bytes;
            }
        }
        true
    }

    /// Every mesh goes (project closed, device lost).
    pub(super) fn clear(&mut self) {
        self.meshes.clear();
        self.bytes = 0;
    }

    /// The bytes the meshes hold.
    pub(super) fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Meshes kept (refusals not counted).
    pub(super) fn len(&self) -> usize {
        self.meshes.values().filter(|c| c.mesh.is_some()).count()
    }

    /// Meshes tessellated so far.
    pub(super) fn made(&self) -> u64 {
        self.made
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(geometry: u64) -> MeshKey {
        MeshKey {
            geometry,
            fill: None,
            stroke: None,
            tolerance: 0,
        }
    }

    /// Refusals count against the bound like meshes do: one past it makes room or isn't kept.
    #[test]
    fn a_refusal_stays_within_the_bound() {
        let mut cache = MeshCache::default();
        cache.refuse(key(1), 1);
        assert_eq!(cache.bytes(), REFUSAL_BYTES);
        // Full: the next refusal evicts the oldest one rather than pass the bound.
        cache.bytes = MAX_MESH_BYTES;
        cache.refuse(key(2), 2);
        assert!(cache.bytes() <= MAX_MESH_BYTES, "{}", cache.bytes());
        assert!(!cache.meshes.contains_key(&key(1)));
    }
}
