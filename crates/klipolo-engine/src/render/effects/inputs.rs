//! The bind groups passes read their input through, kept between frames: an outside texture's (an image, text,
//! overlay video, the frame under an adjustment), or the frame with an overlay-blended layer's texture, made on its
//! first use, and a pair of the stage's targets read together (a transition's two sides). Each goes shortly after its
//! last use, a pair also with its targets.

use super::pipelines::input_group;
use super::{EffectStage, made};

/// An outside texture's input bind group, made on its first use.
pub(super) struct External {
    key: ExternalKey,
    pub(super) group: wgpu::BindGroup,
    pub(super) used: u64,
}

#[derive(PartialEq)]
pub(super) enum ExternalKey {
    Texture(wgpu::Texture),
    View(wgpu::TextureView),
    /// The frame drawn so far and an overlay-blended layer's texture (`Input::Under`).
    Under(wgpu::TextureView, wgpu::Texture),
}

/// Two targets' input bind group (`kl_a`, `kl_b`), made on their first use together and keyed by the targets' ids
/// (indices change as other targets are dropped).
pub(super) struct PairInput {
    a: u64,
    b: u64,
    pub(super) group: wgpu::BindGroup,
    used: u64,
}

impl EffectStage {
    /// The input bind group of an outside texture (or two): kept from an earlier frame, or made now with `views`
    /// (`kl_a`, and `kl_b` or none).
    pub(super) fn external(
        &mut self,
        device: &wgpu::Device,
        key: ExternalKey,
        views: impl FnOnce() -> (wgpu::TextureView, Option<wgpu::TextureView>),
    ) -> usize {
        let now = self.frame;
        if let Some(i) = self.externals.iter().position(|e| e.key == key) {
            self.externals[i].used = now;
            return i;
        }
        let (a, b) = views();
        let group = input_group(
            device,
            &self.input_layout,
            &a,
            b.as_ref().unwrap_or(&self.empty_view),
        );
        made();
        self.externals.push(External {
            key,
            group,
            used: now,
        });
        self.externals.len() - 1
    }

    /// The input bind group of targets `a` and `b` read together: kept from an earlier frame, or made now.
    pub(super) fn pair(&mut self, device: &wgpu::Device, a: usize, b: usize) -> usize {
        let (id_a, id_b) = (self.targets[a].id, self.targets[b].id);
        let now = self.frame;
        if let Some(i) = self.pairs.iter().position(|p| p.a == id_a && p.b == id_b) {
            self.pairs[i].used = now;
            return i;
        }
        let group = input_group(
            device,
            &self.input_layout,
            &self.targets[a].view,
            &self.targets[b].view,
        );
        made();
        self.pairs.push(PairInput {
            a: id_a,
            b: id_b,
            group,
            used: now,
        });
        self.pairs.len() - 1
    }

    /// Drops the pair bind groups last used before frame `oldest`, or whose targets are gone (a bind group would
    /// keep a dropped target's texture alive).
    pub(super) fn evict_pairs(&mut self, oldest: u64) {
        let targets = &self.targets;
        let live = |id: u64| targets.iter().any(|t| t.id == id);
        self.pairs
            .retain(|p| p.used >= oldest && live(p.a) && live(p.b));
    }
}
