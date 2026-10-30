//! How a page atlas's shadow and glow are blurred, worked out once per page on the host
//! so the shader does no transcendental maths: each blur's Gaussian weights, paired so one linear fetch reads two
//! neighbouring texels at their weighted middle (as the built-in `blur.wgsl` does), and, for blurs at least
//! `HALVE_FROM_SIGMA` wide, the blurred masks made at half size (as the built-in blur runs at half size from an 8 px
//! radius): a 2 × 2 average, the two passes there, and a linear upscale into the atlas.
//!
//! A pass reads at most 1 + 2 × `MAX_PAIRS` = 49 texels a blur (the brute-force pass read up to 97, with two `exp()`
//! a texel); at half size it runs on a quarter of the texels.

use arrayvec::ArrayVec;
use bytemuck::{Pod, Zeroable};

/// Pairs of taps a side at most: a pass's taps reach 3σ, spaced a texel apart until that needs more than this, then
/// spread out evenly.
pub const MAX_PAIRS: usize = 24;
/// Blurs this wide (σ in the atlas's pixels) and wider run at half size.
pub const HALVE_FROM_SIGMA: f32 = 4.0;
/// What the 2 × 2 average and the linear upscale add to a half-size blur's variance, px² of the atlas each way (a
/// 2-px box: 4/12; a tent 2 px each side: 4/6), taken off its σ so the shadow and glow keep their size.
const HALVING_VARIANCE: f32 = 1.0;

/// One blur's taps in a pass: the centre texel's weight and each pair's offset (texels of the pass's texture) and
/// weight, read on both sides. No pairs and a centre of 1 keep the source.
#[derive(Clone, Debug, PartialEq)]
pub struct Taps {
    pub centre: f32,
    pub pairs: ArrayVec<[f32; 2], MAX_PAIRS>,
}

impl Taps {
    /// The source as it is.
    pub fn keep() -> Self {
        Self {
            centre: 1.0,
            pairs: ArrayVec::new(),
        }
    }

    /// Texels a pass reads for this blur.
    pub fn fetches(&self) -> usize {
        1 + 2 * self.pairs.len()
    }
}

/// The taps of a Gaussian of `sigma` texels (of the pass's texture), out to 3σ, normalised. A σ of about 0 keeps the
/// source.
pub fn taps(sigma: f32) -> Taps {
    let mut t = Taps::keep();
    if sigma.is_nan() || sigma <= 1e-3 {
        return t;
    }
    let reach = (3.0 * sigma).ceil();
    let stride = (reach / (2 * MAX_PAIRS) as f32).max(1.0);
    let n = ((reach / stride).ceil() as usize).min(2 * MAX_PAIRS);
    let w = |k: usize| {
        let x = k as f32 * stride;
        (-x * x / (2.0 * sigma * sigma)).exp()
    };
    let mut total = 1.0;
    for k in (1..=n).step_by(2) {
        let (w1, w2) = (w(k), if k < n { w(k + 1) } else { 0.0 });
        let pair = w1 + w2;
        if pair <= 0.0 {
            break;
        }
        let at = (k as f32 * w1 + (k + 1) as f32 * w2) / pair * stride;
        t.pairs.push([at, pair]);
        total += 2.0 * pair;
    }
    t.centre = 1.0 / total;
    for pair in &mut t.pairs {
        pair[1] /= total;
    }
    t
}

/// How a page's blurs run.
#[derive(Clone, Debug, PartialEq)]
pub struct Blurs {
    /// 1, or 2: the blurred masks are made at half size and upscaled into the atlas.
    pub down: u32,
    /// The shadow's taps in each pass (`keep` when it isn't blurred: a hard shadow is the silhouette).
    pub shadow: Taps,
    /// The glow's, or None when the style has no glow (the atlas's glow mask stays empty).
    pub glow: Option<Taps>,
    /// Which blurs the passes make (the others are what the pack pass wrote: the silhouette, the fill).
    pub shadow_blurred: bool,
    pub glow_blurred: bool,
}

/// The blurs of a page whose shadow's σ is `shadow` and glow's `glow` (−1: none), atlas px (`cells::blur_sigmas`), on
/// an atlas of `size`; None when nothing is blurred (the pack pass writes every mask).
pub fn blurs(shadow: f32, glow: f32, size: (u32, u32)) -> Option<Blurs> {
    let shadow_blurred = shadow > 1e-3;
    let glow_blurred = glow > 1e-3;
    if !shadow_blurred && !glow_blurred {
        return None;
    }
    let wide = |s: f32, blurred: bool| !blurred || s >= HALVE_FROM_SIGMA;
    let down =
        if wide(shadow, shadow_blurred) && wide(glow, glow_blurred) && size.0 >= 4 && size.1 >= 4 {
            2
        } else {
            1
        };
    let at = |s: f32| {
        if down == 2 {
            (s * s - HALVING_VARIANCE).max(0.0).sqrt() / 2.0
        } else {
            s
        }
    };
    Some(Blurs {
        down,
        shadow: if shadow_blurred {
            taps(at(shadow))
        } else {
            Taps::keep()
        },
        glow: (glow >= 0.0).then(|| {
            if glow_blurred {
                taps(at(glow))
            } else {
                Taps::keep()
            }
        }),
        shadow_blurred,
        glow_blurred,
    })
}

/// `captions.wgsl`'s `Blur` uniform: a pass's direction, each blur's pair count and centre weight, whether the style
/// glows, and the pairs (the shadow's in the first `MAX_PAIRS / 2` rows, the glow's in the rest, two a row).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct Uniform {
    dir: [f32; 2],
    pairs: [u32; 2],
    centre: [f32; 2],
    glow: f32,
    _pad: f32,
    taps: [[f32; 4]; MAX_PAIRS],
}

impl Uniform {
    /// The pack pass's: only whether the style glows (its fill is then the glow's unblurred mask).
    pub(super) fn pack(glows: bool) -> Self {
        Self {
            glow: if glows { 1.0 } else { 0.0 },
            ..Self::zeroed()
        }
    }

    /// One blur pass of `blurs` along `dir`.
    pub(super) fn pass(blurs: &Blurs, dir: [f32; 2]) -> Self {
        let mut u = Self {
            dir,
            glow: if blurs.glow.is_some() { 1.0 } else { 0.0 },
            ..Self::zeroed()
        };
        let half = MAX_PAIRS / 2;
        let keep = Taps::keep();
        let glow = blurs.glow.as_ref().unwrap_or(&keep);
        for (b, (taps, row)) in [(&blurs.shadow, 0), (glow, half)].into_iter().enumerate() {
            u.pairs[b] = taps.pairs.len() as u32;
            u.centre[b] = taps.centre;
            for (i, [at, w]) in taps.pairs.iter().enumerate() {
                let cell = &mut u.taps[row + i / 2];
                let k = (i % 2) * 2;
                cell[k] = *at;
                cell[k + 1] = *w;
            }
        }
        u
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pass reads at most 49 texels a blur, whatever its σ (the brute-force pass read 97 at the widest), its
    /// weights sum to 1, and a pair's fetch lands between its two texels.
    #[test]
    fn a_blur_pass_reads_at_most_49_texels_a_blur() {
        for (sigma, pairs) in [
            (0.0, 0),
            (0.5, 1),
            (2.0, 3),
            (4.2, 7),
            (10.5, 16),
            (16.0, 24),
            (21.0, 24),
            (96.0, 24),
        ] {
            let t = taps(sigma);
            assert_eq!(t.pairs.len(), pairs, "σ {sigma}");
            assert!(t.fetches() <= 49, "σ {sigma}: {} fetches", t.fetches());
            let total = t.centre + 2.0 * t.pairs.iter().map(|p| p[1]).sum::<f32>();
            assert!(
                (total - 1.0).abs() < 1e-5,
                "σ {sigma}: weights sum to {total}"
            );
            let stride = ((3.0 * sigma).ceil() / 48.0).max(1.0);
            for (i, [at, _]) in t.pairs.iter().enumerate() {
                let k = (2 * i + 1) as f32 * stride;
                assert!(
                    *at >= k - 1e-4 && *at <= k + stride + 1e-4,
                    "σ {sigma}: pair {i} at {at}"
                );
            }
        }
    }

    /// Blurs at least 4 px wide run at half size, their σ there halved less the halving's own blur; a narrower
    /// one keeps the whole atlas at full size, and a page with no blur runs no pass.
    #[test]
    fn wide_blurs_run_at_half_size_and_keep_their_size() {
        // Arcade at 1080 × 1920: a glow of σ 21 px, no shadow.
        let b = blurs(0.0, 21.0, (1560, 200)).unwrap();
        assert_eq!(b.down, 2);
        assert_eq!(b.shadow, Taps::keep());
        assert!(!b.shadow_blurred && b.glow_blurred);
        let expected = taps((21.0f32 * 21.0 - 1.0).sqrt() / 2.0);
        assert_eq!(b.glow.as_ref(), Some(&expected));
        assert!(expected.fetches() <= 33, "{} fetches", expected.fetches());
        // Karaoke at 1080 × 1920: a shadow of σ 3.9 px stays at full size, and no glow.
        let b = blurs(3.9, -1.0, (1560, 200)).unwrap();
        assert_eq!((b.down, b.glow.is_none()), (1, true));
        // Loud: a hard shadow and a keyword's glow; one with neither blurs nothing.
        assert_eq!(blurs(0.0, 19.5, (900, 200)).unwrap().down, 2);
        assert_eq!(blurs(0.0, -1.0, (900, 200)), None);
    }
}
