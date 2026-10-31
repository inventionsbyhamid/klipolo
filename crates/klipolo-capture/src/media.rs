//! What sources produce: encoded video packets and PCM blocks, each stamped on the host clock
//! (`klipolo_platform::clock::host_ns`). The cursor's events are `cursor::CursorEvent`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A source's id in a session and its folder name: "screen", "camera", "mic", "gameSound", "computerSound".
pub type SourceId = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Codec {
    H264,
    Hevc,
}

/// A video stream as its encoder describes it. The journal keeps it (`RunStarted`, with `config` as hex), so recovery
/// can rebuild a damaged file head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub codec: Codec,
    /// The `avcC` or `hvcC` record's body.
    #[serde(with = "hex_bytes")]
    pub config: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// One encoded frame: AVCC, with a 4-byte length before each NAL unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedPacket {
    /// When the frame was captured, on the host clock.
    pub host_ns: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFormat {
    pub rate: u32,
    pub channels: u16,
}

/// A block of PCM as the device delivered it.
#[derive(Clone, Debug, PartialEq)]
pub struct PcmBlock {
    /// When the block's first frame was captured, on the host clock.
    pub host_ns: u64,
    /// Interleaved samples in [-1, 1].
    pub samples: Vec<f32>,
}

impl PcmBlock {
    /// The frames it holds for `channels`; refused when its sample count isn't a whole number of them (or there are
    /// no channels).
    pub fn whole_frames(&self, channels: u16) -> Result<u64, PartialBlock> {
        let n = usize::from(channels);
        if n == 0 || !self.samples.len().is_multiple_of(n) {
            return Err(PartialBlock {
                samples: self.samples.len(),
                channels,
            });
        }
        Ok((self.samples.len() / n) as u64)
    }
}

/// A sound block whose sample count isn't a whole number of frames. Written, it would shift every
/// later frame's channels (left becomes right) and leave a torn frame mid-file, so it is refused whole: the core drops
/// it and reports `coded()` once per run as a mild health note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartialBlock {
    pub samples: usize,
    pub channels: u16,
}

impl PartialBlock {
    /// `capture.note.soundBlock` ("A moment of sound was dropped."; milder than `capture.error.os`), with
    /// the block's shape for the log.
    pub fn coded(&self) -> klipolo_model::coded::CodedError {
        crate::english::msg!(
            "capture.note.soundBlock",
            samples = self.samples,
            channels = u32::from(self.channels)
        )
    }
}

/// Bytes as a lowercase hex string in JSON.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        hex::decode(String::deserialize(d)?).map_err(D::Error::custom)
    }
}
