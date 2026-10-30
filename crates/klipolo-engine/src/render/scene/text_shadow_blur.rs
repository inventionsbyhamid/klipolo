//! A baked text shadow's blur, drawn small: as the letters' blur is (`text_blur.rs`),
//! the silhouette is sampled down by a power-of-two step that leaves its Gaussian three to six texels wide there, blurred
//! across and down at that size, and read back up by the text pass's bilinear fetch. So a shadow's bake costs about one
//! read per texel of its cell, whatever its blur: a blur of 256 took seconds at full resolution. Each small tap weighs
//! what the built-in blur's kernel weighs over the atlas texels it spans (`binned`), so it ends where that kernel ends
//! and sums to one as it does; its deviation is that kernel's less what the box sample and the binning add. Drawn so,
//! it is the full-size shadow within about 5/255 across the whole cell (about 4.6/255 at worst, 
//! one reach in from the silhouette's edge, where the small kernel's last bin and the bilinear read just catch the
//! blank edge; at most a 1.8 % dip of the shadow's alpha; sampling the Gaussian tap by tap there was up to 7/255 off:
//! its end fell half a small texel past the kernel's). A Gaussian under six texels is blurred texel for texel, as
//! before.

/// How a shadow of `sigma` atlas texels is blurred: a texel every `step` atlas texels, and its weights there (the middle
/// tap's and each one out, summing to one both ways).
pub(super) struct Small {
    pub step: u32,
    pub weights: Vec<f32>,
}

impl Small {
    pub(super) fn of(sigma: f32) -> Self {
        if sigma < 6.0 {
            return Self {
                step: 1,
                weights: weights(sigma),
            };
        }
        let step = (sigma / 3.0).log2().floor().exp2() as u32;
        Self {
            step,
            weights: binned(sigma, step),
        }
    }

    /// Its cell's room past the silhouette's texels, in its own texels: the blur's reach and one.
    pub(super) fn margin(&self) -> u32 {
        self.weights.len() as u32
    }

    /// Its cell's size, in its own texels, for a silhouette of `w` × `h` atlas texels.
    pub(super) fn size(&self, (w, h): (u32, u32)) -> (u32, u32) {
        let m = 2 * self.margin();
        (w.div_ceil(self.step) + m, h.div_ceil(self.step) + m)
    }
}

/// The built-in blur's reach for a Gaussian of `sigma` texels: twice the deviation, in whole pairs.
fn reach_of(sigma: f32) -> usize {
    ((2.0 * sigma).ceil() as usize).div_ceil(2) * 2
}

/// A Gaussian of `sigma` texels out to `reach` each way, summing to one.
fn gaussian(sigma: f32, reach: usize) -> Vec<f32> {
    let k2 = 1.0 / (2.0 * sigma * sigma);
    let mut w: Vec<f32> = (0..=reach)
        .map(|k| (-((k * k) as f32) * k2).exp())
        .collect();
    let total = w[0] + 2.0 * w[1..].iter().sum::<f32>();
    w.iter_mut().for_each(|v| *v /= total);
    w
}

/// The built-in blur's weights for a Gaussian of `sigma` texels: the middle tap's and each one out to its reach.
pub(super) fn weights(sigma: f32) -> Vec<f32> {
    if sigma < 0.05 {
        return vec![1.0];
    }
    gaussian(sigma, reach_of(sigma))
}

/// The built-in blur's kernel for `sigma` atlas texels as taps `step` texels apart: each tap what the kernel weighs over
/// the atlas texels its span covers, out to the kernel's own reach, the kernel's deviation less a sixth of the step
/// squared (what the box sample and the binning add).
fn binned(sigma: f32, step: u32) -> Vec<f32> {
    let reach = reach_of(sigma);
    let s = step as f32;
    let full = gaussian((sigma * sigma - s * s / 6.0).max(1e-6).sqrt(), reach);
    let r = reach as f32;
    let taps = ((r + 0.5) / s - 0.5).ceil().max(0.0) as usize;
    (0..=taps)
        .map(|k| {
            let (lo, hi) = (k as f32 * s - s / 2.0, k as f32 * s + s / 2.0);
            let (from, to) = ((lo - 0.5).floor().max(-r), (hi + 0.5).ceil().min(r));
            (from as i64..=to as i64)
                .map(|j| {
                    let j = j as f32;
                    let overlap = (hi.min(j + 0.5) - lo.max(j - 0.5)).max(0.0);
                    full[j.abs() as usize] * overlap
                })
                .sum()
        })
        .collect()
}

/// The silhouette `g` (`w` × `h` texels: `g(x, y)` in 0–1) blurred small by `small` into `out` (an image `stride` bytes
/// wide) at `at`, its cell `small.size` big: the mean of each step × step box, then across, then down, transparent past
/// the silhouette. The taps and reads it took.
pub(super) fn blur_small(
    g: impl Fn(u32, u32) -> f32,
    (w, h): (u32, u32),
    small: &Small,
    (out, stride): (&mut [u8], u32),
    at: [u32; 2],
) -> u64 {
    let (s, m) = (small.step, small.margin() as i64);
    let (iw, ih) = (w.div_ceil(s), h.div_ceil(s));
    let (cw, ch) = small.size((w, h));
    // Sampled down: each texel the mean of its box (a box past the silhouette's edge counts what is past it as clear).
    let mut boxed = vec![0f32; (iw * ih) as usize];
    for j in 0..ih {
        for i in 0..iw {
            let mut sum = 0.0;
            for y in j * s..((j + 1) * s).min(h) {
                for x in i * s..((i + 1) * s).min(w) {
                    sum += g(x, y);
                }
            }
            boxed[(j * iw + i) as usize] = sum / (s * s) as f32;
        }
    }
    let wt = &small.weights;
    let reach = wt.len() as i64 - 1;
    let (iw, ih, cw, ch) = (iw as i64, ih as i64, i64::from(cw), i64::from(ch));
    // Across: each row of it, at every column of the cell.
    let mut across = vec![0f32; (cw * ih) as usize];
    for y in 0..ih {
        for cx in 0..cw {
            let x = cx - m;
            let mut sum = 0.0;
            for k in (-reach).max(-x)..=reach.min(iw - 1 - x) {
                sum += wt[k.unsigned_abs() as usize] * boxed[(y * iw + x + k) as usize];
            }
            across[(y * cw + cx) as usize] = sum;
        }
    }
    // Down: every texel of the cell.
    for cy in 0..ch {
        let y = cy - m;
        for cx in 0..cw {
            let mut sum = 0.0;
            for k in (-reach).max(-y)..=reach.min(ih - 1 - y) {
                sum += wt[k.unsigned_abs() as usize] * across[((y + k) * cw + cx) as usize];
            }
            let o = (i64::from(at[1]) + cy) * i64::from(stride) + i64::from(at[0]) + cx;
            out[o as usize] = (sum.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    let taps = (2 * reach + 1) as u64;
    u64::from(w) * u64::from(h) + (cw * ih + cw * ch) as u64 * taps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in blur's weights: out to twice the deviation in whole pairs, summing to one both ways.
    #[test]
    fn a_shadows_weights_are_the_built_in_blurs() {
        let w = weights(3.0);
        assert_eq!(w.len(), 7, "taps 1..=6: twice the deviation");
        let total = w[0] + 2.0 * w[1..].iter().sum::<f32>();
        assert!((total - 1.0).abs() < 1e-6);
        assert!((w[1] / w[0] - (-1.0f32 / 18.0).exp()).abs() < 1e-6);
        assert_eq!(weights(0.0), vec![1.0]);
        assert_eq!(Small::of(5.9).step, 1);
        assert_eq!(Small::of(24.0).step, 8);
        assert_eq!(Small::of(256.0).step, 64);
        // Drawn small, the taps still sum to one both ways and end where the full kernel ends (half a tap's span).
        for sigma in [6.0f32, 24.0, 47.0, 256.0] {
            let small = Small::of(sigma);
            let w = &small.weights;
            let total = w[0] + 2.0 * w[1..].iter().sum::<f32>();
            assert!((total - 1.0).abs() < 1e-4, "σ {sigma}: {total}");
            let end = (w.len() as f32 - 0.5) * small.step as f32;
            let reach = weights(sigma).len() as f32 - 0.5;
            assert!(
                end >= reach && end < reach + small.step as f32,
                "σ {sigma}: {end} against {reach}"
            );
        }
    }

    /// Drawn small, a shadow is the one drawn texel for texel within 5/255 at every atlas texel of its cell (the whole
    /// check: the whole grid, not sample points): a 40 × 40 square under Gaussians of 12 and 24 texels, and a
    /// 400 × 100 one under 24, read back bilinearly as the text pass reads it, against the exact separable sum.
    #[test]
    fn a_small_shadow_is_the_full_size_one() {
        for (w, h, sigma) in [(40u32, 40u32, 24.0f32), (40, 40, 12.0), (400, 100, 24.0)] {
            let worst = worst_across_the_cell(w, h, sigma);
            assert!(
                worst <= 5.0 / 255.0,
                "{w} × {h} at σ {sigma}: worst {:.2}/255",
                worst * 255.0
            );
        }
    }

    /// The worst difference between a `w` × `h` square's shadow drawn small and the exact one, over every atlas texel
    /// the full kernel reaches.
    fn worst_across_the_cell(w: u32, h: u32, sigma: f32) -> f32 {
        let g = |x: u32, y: u32| if x < w && y < h { 1.0 } else { 0.0 };
        let small = Small::of(sigma);
        let (cw, ch) = small.size((w, h));
        let mut out = vec![0u8; (cw * ch) as usize];
        blur_small(g, (w, h), &small, (&mut out, cw), [0, 0]);
        let full = weights(sigma);
        let reach = full.len() as i64 - 1;
        // The exact sum along one line at texel `p` of a run `n` long (relative to the square's corner).
        let line = |p: i64, n: i64| -> f32 {
            (-reach..=reach)
                .filter(|k| (0..n).contains(&(p + k)))
                .map(|k| full[k.unsigned_abs() as usize])
                .sum()
        };
        let across: Vec<f32> = (-reach..i64::from(w) + reach)
            .map(|x| line(x, i64::from(w)))
            .collect();
        let down: Vec<f32> = (-reach..i64::from(h) + reach)
            .map(|y| line(y, i64::from(h)))
            .collect();
        let (s, m) = (small.step as f32, small.margin() as f32);
        let read = |x: i64, y: i64| {
            // The small cell's texel space: atlas texel centre (x + 0.5) → (x + 0.5) / s + m.
            let (u, v) = (
                (x as f32 + 0.5) / s + m - 0.5,
                (y as f32 + 0.5) / s + m - 0.5,
            );
            let (u0, v0) = (u.floor().max(0.0), v.floor().max(0.0));
            let (fu, fv) = (u - u0, v - v0);
            let at = |i: f32, j: f32| {
                let (i, j) = ((i as u32).min(cw - 1), (j as u32).min(ch - 1));
                f32::from(out[(j * cw + i) as usize]) / 255.0
            };
            let top = at(u0, v0) * (1.0 - fu) + at(u0 + 1.0, v0) * fu;
            let bottom = at(u0, v0 + 1.0) * (1.0 - fu) + at(u0 + 1.0, v0 + 1.0) * fu;
            top * (1.0 - fv) + bottom * fv
        };
        let mut worst = 0.0f32;
        for (i, a) in across.iter().enumerate() {
            for (j, d) in down.iter().enumerate() {
                let (x, y) = (i as i64 - reach, j as i64 - reach);
                worst = worst.max((read(x, y) - a * d).abs());
            }
        }
        worst
    }
}
