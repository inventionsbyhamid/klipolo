//! What a frame says in numbers, so an agent can tell a black, flat, still or broken shader without
//! seeing it: luma by BT.709 on the gamma-encoded RGB of the frame (a draft's alone frame, before the checkerboard),
//! the share of pixels at least half opaque, the pixels the output check replaced (`audit`), the change from the
//! previous frame sampled (mean absolute luma difference) and, on the last, how far it is from the first (`loop_seam`).
//! One pass over the pixels; a frame's luma plane is kept only until the next frame's change is measured.

use serde::{Deserialize, Serialize};

/// A frame's statistics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameStats {
    pub luma: Luma,
    pub alpha_coverage: f64,
    /// Pixels the output check replaced (NaN or infinite), counted at 128 × 72.
    pub nan_fixed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_from_prev: Option<f64>,
    /// How far the last frame of a loop is from its first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_seam: Option<f64>,
}

/// 0–1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Luma {
    pub mean: f64,
    pub min: f64,
    pub max: f64,
}

/// `rgba` (rows packed, `width` wide) in numbers, with its luma plane for the next frame's change.
pub fn frame_stats(rgba: &[u8], prev: Option<&[f32]>) -> (FrameStats, Vec<f32>) {
    let mut plane = Vec::with_capacity(rgba.len() / 4);
    let (mut sum, mut min, mut max, mut covered) = (0.0f64, f64::MAX, 0.0f64, 0u64);
    for p in rgba.chunks_exact(4) {
        let y = (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
            / 255.0;
        plane.push(y);
        let y = f64::from(y);
        sum += y;
        min = min.min(y);
        max = max.max(y);
        covered += u64::from(p[3] >= 128);
    }
    let n = plane.len().max(1) as f64;
    if plane.is_empty() {
        min = 0.0;
    }
    let stats = FrameStats {
        luma: Luma {
            mean: sum / n,
            min,
            max,
        },
        alpha_coverage: covered as f64 / n,
        nan_fixed: 0,
        change_from_prev: prev.map(|prev| change(prev, &plane)),
        loop_seam: None,
    };
    (stats, plane)
}

/// The mean absolute luma difference of two frames' planes (0 when their sizes differ: never in one request).
pub fn loop_seam(first: &[f32], last: &[f32]) -> f64 {
    change(first, last)
}

fn change(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let total: f64 = a.iter().zip(b).map(|(x, y)| f64::from((x - y).abs())).sum();
    total / a.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_white_and_half_covered_frames_in_numbers() {
        let black = [0u8, 0, 0, 255].repeat(4);
        let (s, plane) = frame_stats(&black, None);
        assert_eq!(
            (s.luma.max, s.alpha_coverage, s.change_from_prev),
            (0.0, 1.0, None)
        );
        let mixed = [[255u8, 255, 255, 255], [0, 0, 0, 0]].concat().repeat(2);
        let (s, plane2) = frame_stats(&mixed, Some(&plane));
        assert!((s.luma.mean - 0.5).abs() < 1e-6 && (s.alpha_coverage - 0.5).abs() < 1e-9);
        assert!((s.change_from_prev.unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(loop_seam(&plane2, &plane2), 0.0);
    }
}
