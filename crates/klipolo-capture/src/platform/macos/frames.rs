//! Generated NV12 frames for the probe and the encoder's tests, so neither ever captures: IOSurface-backed
//! as capture's are, and filled once, at setup, never on a frame path: high-entropy noise (the encoder's worst case,
//! foliage or particles) under a bright bar that moves from one frame to the next.

use cidre::{arc, cv};

use super::pool::nv12_attrs;

/// `count` NV12 (`420v`) IOSurface buffers of `width × height`; fewer if the system won't make them.
pub fn generated_nv12(width: u32, height: u32, count: usize) -> Vec<arc::R<cv::PixelBuf>> {
    let Some(attrs) = nv12_attrs(width, height) else {
        return vec![];
    };
    (0..count)
        .filter_map(|n| {
            let mut buf = cv::PixelBuf::new(
                width as usize,
                height as usize,
                cv::PixelFormat::_420V,
                Some(&attrs),
            )
            .ok()?;
            fill(&mut buf, n, count.max(1)).then_some(buf)
        })
        .collect()
}

/// Noise in both planes, then frame `n`'s bar (a sixteenth of the width, at `n / count` across) in luma.
fn fill(buf: &mut cv::PixelBuf, n: usize, count: usize) -> bool {
    // SAFETY: the buffer is locked for writing while its planes are written, inside their bounds, and unlocked after.
    unsafe {
        if buf.lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT).0 != 0 {
            return false;
        }
        let mut seed =
            0x9E37_79B9_7F4A_7C15_u64 ^ (n as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03);
        for plane in 0..buf.plane_count() {
            let rows = buf.plane_height(plane);
            let stride = buf.plane_bytes_per_row(plane);
            let base = buf.plane_base_address(plane).cast_mut();
            let bytes = std::slice::from_raw_parts_mut(base, rows * stride);
            for chunk in bytes.chunks_mut(8) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let noise = seed.to_le_bytes();
                chunk.copy_from_slice(&noise[..chunk.len()]);
            }
            if plane == 0 {
                let width = buf.plane_width(0);
                let (from, wide) = (width * n / count, (width / 16).max(1));
                for row in bytes.chunks_mut(stride) {
                    let end = (from + wide).min(width);
                    row[from..end].fill(235);
                }
            }
        }
        let _ = buf.unlock_lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT);
    }
    true
}
