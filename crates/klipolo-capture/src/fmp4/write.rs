//! The fragmented MP4 writer: the head, then a `moof`+`mdat` pair every 0.5 s. It only makes bytes; the durable
//! writer puts them in the file and syncs them.

use klipolo_model::time::Us;

use super::boxes::{be32, be64, bx, full};
use super::moov::{TRACK_ID, moov};
use super::{FRAGMENT_US, FragIndex, Index, KEY_FLAGS, OTHER_FLAGS, SampleIndex, TIMESCALE};
use crate::media::{EncodedPacket, StreamInfo};

/// A sample's duration when it is the run's only one, so there is no earlier duration to keep: 1/30 s.
const LONE_DURATION: u32 = TIMESCALE / 30;
/// A fragment also closes at about 0.5 s of frames at the run's rate, or at 16 MiB (0.5 s at 268 Mbps), whatever the
/// timestamps say, so a packet stamped ahead of real time can't hold one open (the
/// cap follows the rate). A kill then loses at most about a second of video even after such a glitch. `new`'s rate is
/// the fastest preset's, 120 fps (61 frames).
const FASTEST_FPS: u32 = 120;
const MAX_FRAGMENT_BYTES: usize = 16 << 20;

/// A whole `moof`+`mdat` pair, ready to append to the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fragment {
    pub bytes: Vec<u8>,
    /// The first sample's run time, as written (90 kHz ticks, rounded to µs).
    pub start_us: Us,
    /// The run time just after the last sample: exactly the next fragment's `start_us`, and never before this one's.
    pub end_us: Us,
}

impl Fragment {
    /// The media it holds, from its own start and end times (never from raw stamps): what the durable writer's queue
    /// counts for it.
    pub fn media_us(&self) -> Us {
        self.end_us - self.start_us
    }
}

/// Turns one run's encoded packets into a fragmented MP4. Memory: the pending 0.5 s of packets (one buffer, reused; at
/// most about 0.5 s of frames or 16 MiB) and the index (12 bytes a sample plus 48 a fragment: about 2.9 MB an hour at 60 fps,
/// 17.6 MB for 6 hours, and about 33 MB for 6 hours at 120 fps). `push` is O(1) amortised, plus one copy of the
/// packet; a fragment costs one allocation of its own size.
#[derive(Clone, Debug)]
pub struct Fmp4Writer {
    info: StreamInfo,
    head: Vec<u8>,
    /// Bytes emitted so far, the head included: where the next fragment starts in the file.
    written: u64,
    sequence: u32,
    /// The samples of the fragment being gathered; the last one's duration is known when the next packet arrives.
    pending: Vec<SampleIndex>,
    data: Vec<u8>,
    first_ticks: u64,
    first_us: Us,
    last_ticks: u64,
    /// The latest known sample duration, which a flushed last sample keeps.
    last_duration: u32,
    /// The earliest tick the next sample may start at, so decode times always increase.
    next_min: u64,
    /// A fragment closes at this many samples (`at_rate`).
    max_samples: usize,
    index: Index,
}

impl Fmp4Writer {
    /// A writer for any rate up to the fastest preset's. The core uses `at_rate`; recovery rebuilds a head with this
    /// (the head doesn't depend on the rate).
    pub fn new(info: &StreamInfo) -> Self {
        Self::at_rate(info, FASTEST_FPS)
    }

    /// A writer for a run at `fps` frames a second: a fragment also closes at about 0.5 s of frames, `fps / 2` rounded
    /// up plus one for jitter (16 at 30 fps, 31 at 60), so a stamp that jumps ahead can't hold it open longer.
    pub fn at_rate(info: &StreamInfo, fps: u32) -> Self {
        let mut head = Vec::with_capacity(1024 + info.config.len());
        bx(&mut head, b"ftyp", |o| {
            o.extend_from_slice(b"iso6");
            be32(o, 0);
            o.extend_from_slice(b"isomiso6mp41");
        });
        moov(&mut head, info, TIMESCALE, 0, true, |o| {
            full(o, b"stts", 0, 0, |o| be32(o, 0));
            full(o, b"stsc", 0, 0, |o| be32(o, 0));
            full(o, b"stsz", 0, 0, |o| {
                be32(o, 0);
                be32(o, 0);
            });
            full(o, b"stco", 0, 0, |o| be32(o, 0));
        });
        Self {
            info: info.clone(),
            written: head.len() as u64,
            head,
            sequence: 1,
            pending: Vec::new(),
            data: Vec::new(),
            first_ticks: 0,
            first_us: 0,
            last_ticks: 0,
            last_duration: 0,
            next_min: 0,
            max_samples: fps.max(1).div_ceil(2) as usize + 1,
            index: Index::default(),
        }
    }

    /// `ftyp` + `moov` (with `mvex`): the file's first bytes, written and synced before any media.
    pub fn head(&self) -> Vec<u8> {
        self.head.clone()
    }

    /// A packet at `pts_us` (run time: 0 at the run's first frame, which is a keyframe). Returns a whole fragment once
    /// the buffered span reaches 0.5 s, or about 0.5 s of frames at the run's rate (`at_rate`) or 16 MiB are waiting;
    /// the arriving packet starts the next one.
    /// Fragments don't wait for keyframes. A time at or before the previous packet's is moved one tick after it.
    pub fn push(&mut self, pts_us: Us, packet: &EncodedPacket) -> Option<Fragment> {
        let ticks = ticks(pts_us).max(self.next_min);
        if let Some(last) = self.pending.last_mut() {
            last.duration = u32::try_from(ticks - self.last_ticks).unwrap_or(u32::MAX);
            self.last_duration = last.duration;
        }
        let full = self.pending.len() >= self.max_samples || self.data.len() >= MAX_FRAGMENT_BYTES;
        let fragment = (!self.pending.is_empty()
            && (pts_us - self.first_us >= FRAGMENT_US || full))
            .then(|| self.close(ticks));
        if self.pending.is_empty() {
            (self.first_ticks, self.first_us) = (ticks, pts_us);
        }
        self.pending.push(SampleIndex {
            size: u32::try_from(packet.data.len()).expect("a video packet under 4 GB"),
            duration: 0,
            key: packet.keyframe,
        });
        self.data.extend_from_slice(&packet.data);
        (self.last_ticks, self.next_min) = (ticks, ticks + 1);
        fragment
    }

    /// At Stop: the last fragment, whose last sample keeps the previous sample's duration. None when nothing waits.
    pub fn flush(&mut self) -> Option<Fragment> {
        let last = self.pending.last_mut()?;
        last.duration = if self.last_duration > 0 {
            self.last_duration
        } else {
            LONE_DURATION
        };
        self.next_min = self.last_ticks + u64::from(last.duration);
        Some(self.close(self.next_min))
    }

    /// Every fragment emitted so far: the remux's sample table (`remux_plan`).
    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// Where the head's `moov` starts: after the `ftyp`.
    pub(crate) fn head_moov_at(&self) -> u64 {
        u64::from(u32::from_be_bytes(
            self.head[..4].try_into().expect("an ftyp"),
        ))
    }

    /// The head plus every fragment emitted: the file's length once they are all written.
    pub fn bytes_written(&self) -> u64 {
        self.written
    }

    /// The pending samples as a fragment that ends at `end_ticks`.
    fn close(&mut self, end_ticks: u64) -> Fragment {
        let n = self.pending.len();
        // moof 8 + mfhd 16 + traf 8 + tfhd 16 + tfdt 20 + trun 20 and 12 a sample.
        let moof_len = 88 + 12 * n;
        let mut bytes = Vec::with_capacity(moof_len + 8 + self.data.len());
        bx(&mut bytes, b"moof", |o| {
            full(o, b"mfhd", 0, 0, |o| be32(o, self.sequence));
            bx(o, b"traf", |o| {
                // default-base-is-moof: offsets count from the moof's first byte.
                full(o, b"tfhd", 0, 0x02_0000, |o| be32(o, TRACK_ID));
                full(o, b"tfdt", 1, 0, |o| be64(o, self.first_ticks));
                // data offset, and each sample's duration, size and flags.
                full(o, b"trun", 0, 0x00_0701, |o| {
                    be32(o, n as u32);
                    be32(o, (moof_len + 8) as u32);
                    for s in &self.pending {
                        be32(o, s.duration);
                        be32(o, s.size);
                        be32(o, if s.key { KEY_FLAGS } else { OTHER_FLAGS });
                    }
                });
            });
        });
        debug_assert_eq!(bytes.len(), moof_len);
        let mdat = u32::try_from(8 + self.data.len()).expect("a fragment under 4 GB");
        be32(&mut bytes, mdat);
        bytes.extend_from_slice(b"mdat");
        bytes.extend_from_slice(&self.data);
        self.data.clear();
        let moof_at = self.written;
        self.index.fragments.push(FragIndex {
            moof_at,
            data_at: moof_at + moof_len as u64 + 8,
            first_dts: self.first_ticks,
            // Exactly sized: the pending buffer keeps its capacity for the next fragment.
            samples: self.pending.drain(..).collect(),
        });
        self.sequence += 1;
        self.written += bytes.len() as u64;
        Fragment {
            bytes,
            start_us: us(self.first_ticks),
            end_us: us(end_ticks),
        }
    }
}

/// µs → 90 kHz ticks, rounded. Taken from each sample's absolute time, so durations never drift.
fn ticks(us: Us) -> u64 {
    u64::try_from(us)
        .unwrap_or(0)
        .saturating_mul(9)
        .saturating_add(50)
        / 100
}

/// 90 kHz ticks → µs, rounded.
fn us(ticks: u64) -> Us {
    (ticks.saturating_mul(100).saturating_add(4) / 9) as Us
}
