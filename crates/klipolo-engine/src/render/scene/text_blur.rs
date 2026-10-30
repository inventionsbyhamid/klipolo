//! The split text units a frame blurs: at most `MAX_BLURRING_UNITS`, counted as their pieces draw,
//! and where each blurring piece's cell is blurred first. A blur is drawn small and read back up, as
//! the built-in blur draws its own (`registry/wgsl/blur.wgsl`): a piece's cell is sampled down by its `step` (the power
//! of two that leaves its deviation between three and six of its texels: a box of step × step atlas texels a texel),
//! blurred across and then down there, each pass a few bilinear fetches that weigh two texels at once with weights that
//! step by constant factors (`Kernel`: no `exp()` a tap), and the text pass reads the result back with one bilinear
//! fetch (`text_blur_pass.rs`, `text.wgsl`). A Gaussian that wide loses nothing at a texel every third of its
//! deviation, and its passes cost the same at any deviation: at most [`MAX_FETCHES`] a texel, where the brute-force
//! blur it replaces took up to 129, at full resolution. A blur is clamped in 1080p pixels, so a 4K
//! export blurs as its preview does. Regions are packed in shelves across the scratch targets, whose height is the
//! smallest row class that holds them; a piece wider than the targets at its step is drawn at a coarser one
//! line blurs to the end of its blur-in); one that doesn't fit, or past [`MAX_BLUR_PIECES`], draws sharp
//! and the frame says `frame.sceneReduced`. Arithmetic into fixed room: nothing allocates.

use std::cell::Cell;

use bytemuck::{Pod, Zeroable};
use klipolo_model::scene::SceneLimits;

/// The most blurring pieces a frame draws (a unit is a piece, or a few when its letters change look).
pub(super) const MAX_BLUR_PIECES: usize = 24;
/// The scratch targets' width, and their heights (row classes: few sizes, so the pool keeps them).
pub(super) const BLUR_WIDTH: u32 = 2048;
pub(super) const BLUR_ROWS: [u32; 5] = [64, 128, 256, 512, 1024];
/// The largest blur a letter draws, a Gaussian in 1080p pixels (m8: the same at every resolution).
pub(super) const MAX_BLUR_PX: f64 = 24.0;
/// The largest deviation drawn, in atlas texels (a 4K frame under a layer scaled twice).
const MAX_SIGMA: f32 = 96.0;
/// A blur keeps at least this many of its texels to its deviation (fewer would show its texels when read back up).
const PER_SIGMA: f32 = 3.0;
/// Its taps reach this many deviations each way, and its region holds three past its cell.
const REACH: f32 = 2.5;
/// The coarsest step a wide piece is drawn at.
const MAX_STEP: f32 = 64.0;
/// Under this deviation in its own texels a blur takes no taps (its neighbours' weights are under 0.4 %, and would
/// underflow further down).
const NO_TAPS: f32 = 0.3;
/// The most fetches a texel of the across or the down pass makes: a deviation under six texels reaches 15 taps each
/// way, two to a fetch, and the middle one.
pub(super) const MAX_FETCHES: u32 = 17;

/// How a deviation of `sigma` atlas texels is drawn: a texel every `step` atlas texels (a power of two), its deviation
/// there (`sigma / step`), its taps each way, the first and the falling ratio of neighbouring weights (w(k + 1) / w(k)
/// starts at `ratio` and is multiplied by `fall` each tap), and its region's margin past the cell in atlas texels (three
/// deviations and a texel, in whole steps).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Kernel {
    pub step: f32,
    pub sigma: f32,
    pub taps: u32,
    pub ratio: f32,
    pub fall: f32,
    pub margin: f32,
}

impl Kernel {
    pub(super) fn of(sigma: f32) -> Self {
        Self::at_least(sigma, 1.0)
    }

    /// [`of`](Self::of), a texel every `least` atlas texels at least (a power of two).
    pub(super) fn at_least(sigma: f32, least: f32) -> Self {
        let sigma = sigma.clamp(1e-3, MAX_SIGMA);
        let step = (sigma / PER_SIGMA)
            .max(1.0)
            .log2()
            .floor()
            .exp2()
            .max(least);
        let s = sigma / step;
        let k2 = 1.0 / (2.0 * s * s);
        let kernel = Self {
            step,
            sigma: s,
            taps: if s < NO_TAPS {
                0
            } else {
                (REACH * s).ceil() as u32
            },
            ratio: (-k2).exp(),
            fall: (-2.0 * k2).exp(),
            margin: (((3.0 * sigma).ceil() + 1.0) / step).ceil() * step,
        };
        debug_assert!(kernel.fetches() <= MAX_FETCHES, "{kernel:?}");
        kernel
    }

    /// The fetches a texel of its across or down pass makes: the middle one and one per pair of taps each way.
    pub(super) fn fetches(&self) -> u32 {
        1 + 2 * self.taps.div_ceil(2)
    }

    /// The fetches its first pass makes for a cell of `texels` atlas texels: one per texel at full size, else one per
    /// two by two (the bilinear mean), over the cell only.
    pub(super) fn sample_fetches(&self, texels: f32) -> f32 {
        if self.step < 1.5 {
            texels
        } else {
            texels / 4.0
        }
    }
}

/// One blurring piece's cell as its passes read it (`text_blur.wgsl`'s `Slot`): its atlas uv rect, its region of the
/// scratch targets in pixels, its step, margin (atlas texels) and taps, its kernel's first ratio and fall, and the
/// scratch targets' size and the atlas's (the scratch's set as the passes run: `size`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct BlurSlot {
    pub cell: [f32; 4],
    pub region: [f32; 4],
    pub misc: [f32; 4],
    pub weights: [f32; 4],
    pub size: [f32; 4],
}

/// Where a blurring piece reads its blur back from: its deviation and step in atlas texels, `a` (the scratch target's
/// pixel is `a.xy + uv × a.zw` for the piece's atlas uv) and `b` (its region's rect, in the scratch target's pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Blurred {
    pub sigma: f32,
    pub step: f32,
    pub a: [f32; 4],
    pub b: [f32; 4],
}

/// The frame's blurring units and pieces.
#[derive(Debug)]
pub(super) struct BlurBudget {
    units: Cell<usize>,
    last: Cell<(usize, u16)>,
    over: Cell<bool>,
    slots: [Cell<BlurSlot>; MAX_BLUR_PIECES],
    count: Cell<usize>,
    /// The shelf being filled: its left, its top and its height, in scratch pixels.
    shelf: Cell<[f32; 3]>,
    /// The texture fetches the frame's blur passes make (the performance numbers).
    fetches: Cell<f64>,
}

impl Default for BlurBudget {
    fn default() -> Self {
        Self {
            units: Cell::new(0),
            last: Cell::new((usize::MAX, 0)),
            over: Cell::new(false),
            slots: std::array::from_fn(|_| Cell::new(BlurSlot::default())),
            count: Cell::new(0),
            shelf: Cell::new([0.0; 3]),
            fetches: Cell::new(0.0),
        }
    }
}

impl BlurBudget {
    pub(super) fn reset(&self) {
        self.units.set(0);
        self.last.set((usize::MAX, 0));
        self.over.set(false);
        self.count.set(0);
        self.shelf.set([0.0; 3]);
        self.fetches.set(0.0);
    }

    /// Whether node `node`'s unit `unit` may blur this frame.
    pub(super) fn allow(&self, node: usize, unit: u16) -> bool {
        if self.last.get() == (node, unit) {
            return true;
        }
        if self.units.get() >= SceneLimits::MAX_BLURRING_UNITS {
            self.over.set(true);
            return false;
        }
        self.units.set(self.units.get() + 1);
        self.last.set((node, unit));
        true
    }

    /// A region for the cell at `uv` (atlas uv) laid out at `rect` (box pixels), blurred by `sigma` box pixels, in an
    /// atlas of `atlas` texels: None (the piece draws sharp) past the room.
    pub(super) fn place(
        &self,
        uv: [f32; 4],
        rect: [f32; 4],
        sigma: f32,
        atlas: (u32, u32),
    ) -> Option<Blurred> {
        let (aw, ah) = (atlas.0.max(1) as f32, atlas.1.max(1) as f32);
        let (cw, ch) = ((uv[2] - uv[0]) * aw, (uv[3] - uv[1]) * ah);
        let per_px = cw / (rect[2] - rect[0]).max(1e-3);
        let sigma = (sigma * per_px).clamp(0.0, MAX_SIGMA);
        let k = self.count.get();
        if !(sigma > 0.05 && cw > 0.0 && ch > 0.0) || k >= MAX_BLUR_PIECES {
            self.over.set(self.over.get() || k >= MAX_BLUR_PIECES);
            return None;
        }
        // Its region: the cell grown by its margin each way, a texel a step; coarser while it is wider than the scratch
        // targets.
        let mut kernel = Kernel::of(sigma);
        let width = |k: &Kernel| ((cw + 2.0 * k.margin) / k.step).ceil();
        while width(&kernel) > BLUR_WIDTH as f32 && kernel.step < MAX_STEP {
            kernel = Kernel::at_least(sigma, kernel.step * 2.0);
        }
        let (step, margin) = (kernel.step, kernel.margin);
        let w = width(&kernel);
        let h = ((ch + 2.0 * margin) / step).ceil();
        let (sw, sh) = (BLUR_WIDTH as f32, BLUR_ROWS[BLUR_ROWS.len() - 1] as f32);
        let [mut x, mut y, mut tall] = self.shelf.get();
        if x + w > sw {
            (x, y, tall) = (0.0, y + tall, 0.0);
        }
        if w > sw || y + h > sh {
            self.over.set(true);
            return None;
        }
        self.shelf.set([x + w, y, tall.max(h)]);
        self.slots[k].set(BlurSlot {
            cell: uv,
            region: [x, y, x + w, y + h],
            misc: [step, margin, kernel.taps as f32, 0.0],
            weights: [kernel.ratio, kernel.fall, 0.0, 0.0],
            size: [0.0, 0.0, aw, ah],
        });
        self.count.set(k + 1);
        let fetches = kernel.sample_fetches(cw * ch) + 2.0 * w * h * kernel.fetches() as f32;
        self.fetches.set(self.fetches.get() + f64::from(fetches));
        Some(Blurred {
            sigma,
            step,
            a: [
                x + (margin - uv[0] * aw) / step,
                y + (margin - uv[1] * ah) / step,
                aw / step,
                ah / step,
            ],
            b: [x, y, x + w, y + h],
        })
    }

    /// The scratch targets' rows this frame: the smallest class holding its shelves.
    pub(super) fn rows(&self) -> u32 {
        let [_, y, tall] = self.shelf.get();
        let used = (y + tall).ceil() as u32;
        BLUR_ROWS
            .into_iter()
            .find(|&r| used <= r)
            .unwrap_or(BLUR_ROWS[BLUR_ROWS.len() - 1])
    }

    /// The frame's blurring pieces' slots, in order.
    pub(super) fn slots(&self) -> impl Iterator<Item = BlurSlot> + '_ {
        self.slots[..self.count.get()].iter().map(Cell::get)
    }

    pub(super) fn count(&self) -> usize {
        self.count.get()
    }

    /// A unit or a piece past the caps drew sharp.
    pub(super) fn over(&self) -> bool {
        self.over.get()
    }

    /// The texture fetches the frame's three blur passes make (the text pass reads each blurring fragment once more).
    pub(super) fn fetches(&self) -> u64 {
        self.fetches.get() as u64
    }
}

#[cfg(test)]
#[path = "text_blur_tests.rs"]
mod tests;
