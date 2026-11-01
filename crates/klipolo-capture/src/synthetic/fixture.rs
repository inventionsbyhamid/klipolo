//! Real encoded video for tests: the packets of a committed fixture (`tests/fixtures/gop-*.mp4`, made once by
//! `scripts/gen-capture-fixtures.sh` with the distro ffmpeg; never linked), read back by the fragmented MP4 reader. Unlike
//! the synthetic packets, they decode, so the writer and the remux are checked with Klipolo's FFmpeg.

use std::path::Path;

use crate::fmp4::read;
use crate::media::{EncodedPacket, StreamInfo};

#[derive(Clone, Debug)]
pub struct FixtureVideo {
    pub info: StreamInfo,
    /// Each frame's packet, in decode order: (keyframe, AVCC bytes).
    pub frames: Vec<(bool, Vec<u8>)>,
}

impl FixtureVideo {
    /// Panics when the fixture is missing or unreadable (a test setup error).
    pub fn load(path: &Path) -> Self {
        let bytes =
            std::fs::read(path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        let (track, samples) =
            read::samples(&bytes).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        Self {
            info: track.info,
            frames: samples
                .into_iter()
                .map(|(_, _, key, data)| (key, data))
                .collect(),
        }
    }

    /// `count` packets, frame `i` captured at `start_ns + i × 10⁹ / fps`. The fixture's frames repeat from its first,
    /// a keyframe, so any count decodes.
    pub fn packets(&self, count: u64, start_ns: u64, fps: u32) -> Vec<EncodedPacket> {
        (0..count)
            .map(|i| {
                let (keyframe, data) = &self.frames[(i % self.frames.len() as u64) as usize];
                EncodedPacket {
                    host_ns: start_ns
                        + (u128::from(i) * 1_000_000_000 / u128::from(fps.max(1))) as u64,
                    keyframe: *keyframe,
                    data: data.clone(),
                }
            })
            .collect()
    }
}
