//! `Cutter`: one file's thumbnail chunks cut during one scan, and its keyframes recorded for the index.
//!
//! A run reads keyframes in order from a seek point. The first keyframe of each one-second slot is decoded (at most
//! one decode per slot, whatever the GOP); a slot without one shows the nearest earlier tile. A chunk is written once
//! the run has read past its end (or reached the end of the file) having entered it from its start; a chunk with no
//! keyframe of its own is written from the last tile before it.

use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ffmpeg::format::stream::Disposition;
use klipolo_model::time::Us;
use klipolo_platform::durable::{DiskReason, classify};

use super::tile::Video;

mod frames;
mod slots;
use crate::log_name;
use crate::media::events::MediaKind;
use crate::media::key::{CacheKind, stem};
use crate::media::keyframes::{self, KeyRecorder, StoredKeys};
use crate::media::sprite::{Sprite, chunk_of, chunk_start_us, chunks_for, slot_of};
use crate::media::store::Shared;
use crate::media::thumbs::{Fill, has_chunk, poster_in, write_chunk, write_poster};

/// Thumbnail events for one file at most this often while a scan runs (the store sends a trailing one at its end).
const EVENT_EVERY: Duration = Duration::from_millis(500);

/// The current run of packets.
#[derive(Default)]
struct Run {
    /// It began at the file's first packet.
    from_start: bool,
    /// The first keyframe it read, and the latest.
    first_us: Option<Us>,
    last_us: Us,
    /// The slot of the last keyframe it decoded (one decode per slot).
    decoded_slot: Option<u64>,
}

pub(super) struct Cutter {
    shared: Arc<Shared>,
    path: String,
    stem: String,
    video: Option<Video>,
    /// How many chunks the file fills.
    chunks: u32,
    /// Chunks to write (missing on disk when the cutter attached).
    need: BTreeSet<u32>,
    /// The keyframe index isn't stored yet.
    want_keys: bool,
    /// The poster isn't on disk yet: it is cut from the first picture chunk 0 gets.
    want_poster: bool,
    keys: KeyRecorder,
    meta: Option<StoredKeys>,
    sprite: Option<Sprite>,
    run: Run,
    /// A write failed (a full disk), or there's nothing to do: the cutter does nothing more.
    stopped: bool,
    logged: bool,
    wrote: bool,
    decoded: u32,
    last_event: Option<Instant>,
}

impl Cutter {
    /// None when the file can't be read.
    pub(super) fn new(shared: Arc<Shared>, path: &str) -> Option<Cutter> {
        Some(Cutter {
            stem: stem(path, &CacheKind::Thumbs)?,
            shared,
            path: path.to_owned(),
            video: None,
            chunks: 0,
            need: BTreeSet::new(),
            want_keys: false,
            want_poster: false,
            keys: KeyRecorder::default(),
            meta: None,
            sprite: None,
            run: Run::default(),
            stopped: false,
            logged: false,
            wrote: false,
            decoded: 0,
            last_event: None,
        })
    }

    pub(super) fn stopped(&self) -> bool {
        self.stopped
    }

    /// Wrote a chunk or the keyframe index.
    pub(super) fn wrote(&self) -> bool {
        self.wrote
    }

    /// Keyframes sent to the decoder.
    pub(super) fn decoded(&self) -> u32 {
        self.decoded
    }

    pub(super) fn intra_only(&self) -> bool {
        self.video.as_ref().is_some_and(|v| v.intra_only)
    }

    pub(super) fn want_keys(&self) -> bool {
        self.want_keys
    }

    /// Source time of a pts of its stream (None before it attached).
    pub(super) fn us(&self, pts: i64) -> Option<Us> {
        self.video.as_ref().map(|v| v.us(pts))
    }

    /// The video stream to read in `input` (opening its decoder); None: nothing to do in this file.
    pub(super) fn attach(&mut self, input: &ffmpeg::format::context::Input) -> Option<usize> {
        let stream = input.streams().best(ffmpeg::media::Type::Video)?;
        if stream.disposition().contains(Disposition::ATTACHED_PIC) {
            return None;
        }
        let index = stream.index();
        let duration_us = if input.duration() > 0 {
            input.duration()
        } else {
            (stream.duration().max(0) as f64 * f64::from(stream.time_base()) * 1e6) as Us
        };
        let root = &self.shared.root;
        self.chunks = chunks_for(duration_us);
        self.need = (0..self.chunks)
            .filter(|&c| !has_chunk(root, &self.stem, c))
            .collect();
        self.want_keys = !keyframes::has(root, &self.stem);
        // Only with chunk 0: a file whose first chunk is cut already keeps its tile as its picture.
        self.want_poster = self.need.contains(&0) && poster_in(root, &self.stem).is_none();
        if self.need.is_empty() && !self.want_keys {
            self.stopped = true;
            return None;
        }
        match Video::open(input, index) {
            Ok((video, meta)) => {
                self.video = Some(video);
                self.meta = Some(meta);
                Some(index)
            }
            Err(e) => {
                self.warn(&e);
                self.stopped = true;
                None
            }
        }
    }

    fn warn(&mut self, message: &str) {
        if !self.logged {
            self.logged = true;
            tracing::warn!(file = %log_name(&self.path), "{message}");
        }
    }

    /// A write failed: a full disk stops thumbnails quietly (and the store stops scanning); anything else is logged
    /// once. Either way this cutter does nothing more.
    fn failed(&mut self, e: &io::Error) {
        self.stopped = true;
        if classify(e) == DiskReason::Full {
            self.shared.disk_full.store(true, Ordering::SeqCst);
        } else {
            self.warn(&format!("thumbnails not written: {e}"));
        }
    }

    fn notify(&mut self) {
        if self.last_event.is_some_and(|at| at.elapsed() < EVENT_EVERY) {
            return;
        }
        self.last_event = Some(Instant::now());
        self.shared.emit(&self.path, MediaKind::Thumbs);
    }

    /// The current run read every keyframe of `chunk` from its start.
    fn entered_whole(&self, chunk: u32) -> bool {
        self.run.from_start
            || self
                .run
                .first_us
                .is_some_and(|f| f <= chunk_start_us(chunk))
    }

    /// Writes `sprite` (holes filled) when its chunk is wanted, not on disk yet and was entered whole.
    fn write(&mut self, sprite: &mut Sprite) {
        let chunk = sprite.chunk;
        if !self.need.contains(&chunk) || self.stopped {
            return;
        }
        if has_chunk(&self.shared.root, &self.stem, chunk) {
            self.need.remove(&chunk);
            return;
        }
        if !self.entered_whole(chunk) || sprite.is_empty() {
            return;
        }
        sprite.fill_holes();
        let steps = &*self.shared.steps;
        match write_chunk(steps, &self.shared.root, &self.stem, sprite, Fill::Keys) {
            Ok(()) => {
                self.need.remove(&chunk);
                self.wrote = true;
                self.notify();
            }
            Err(e) => self.failed(&e),
        }
    }

    /// Writes the decoder's current frame as the poster (once, whether it was written or not).
    fn poster(&mut self) {
        self.want_poster = false;
        let Some((size, (planes, strides))) = self.video.as_mut().and_then(|v| v.poster_planes())
        else {
            return;
        };
        let steps = &*self.shared.steps;
        let written = write_poster(steps, &self.shared.root, &self.stem, size, planes, strides);
        // A full disk stops the scan as a chunk's would; anything else costs the poster only, never the tiles.
        match written {
            Err(e) if classify(&e) == DiskReason::Full => self.failed(&e),
            Err(e) => self.warn(&format!("poster not written: {e}")),
            Ok(()) => {}
        }
    }

    /// The chunks after `last`'s and before `until` have no keyframe of their own: each shows `last`'s final tile.
    fn bridge(&mut self, last: &Sprite, until: u32) {
        for chunk in last.chunk + 1..until.min(self.chunks) {
            if self.need.contains(&chunk) {
                let mut gap = Sprite::new(chunk, last.tile());
                gap.carry_from(last);
                self.write(&mut gap);
            }
        }
    }

    /// A run of packets starts at `from_us` (0: the file's first packet; else after a seek there).
    pub(super) fn begin(&mut self, from_us: Us) {
        if let Some(video) = self.video.as_mut() {
            video.decoder.flush();
        }
        self.sprite = None;
        self.run = Run {
            from_start: from_us <= 0,
            ..Run::default()
        };
        self.keys.begin(from_us <= 0);
    }

    /// One keyframe of the run, recorded for the index; decoded when `decode` and it is its slot's first.
    pub(super) fn packet(&mut self, packet: &ffmpeg::Packet, decode: bool) {
        if self.stopped || !packet.is_key() {
            return;
        }
        // As `KeyframeIndex::build` reads it.
        self.keys.key(packet.pts().unwrap_or(0));
        let Some(us) = packet
            .pts()
            .or_else(|| packet.dts())
            .and_then(|pts| self.us(pts))
        else {
            return;
        };
        self.run.first_us.get_or_insert(us);
        self.run.last_us = self.run.last_us.max(us);
        let slot = slot_of(us);
        if decode && !self.need.is_empty() && self.run.decoded_slot != Some(slot) {
            self.run.decoded_slot = Some(slot);
            self.decode(packet, None);
        }
    }

    /// The run ended: at the end of the file (`eof`), or early. Every chunk it read past (all of them at the end of
    /// the file) is complete.
    pub(super) fn end(&mut self, eof: bool) {
        if self.video.is_none() {
            return;
        }
        self.drain(None);
        let until = if eof {
            self.chunks
        } else {
            chunk_of(slot_of(self.run.last_us))
        };
        if let Some(mut sprite) = self.sprite.take()
            && sprite.chunk < until
        {
            self.write(&mut sprite);
            self.bridge(&sprite, until);
        }
        self.keys.end(eof);
        self.store_keys();
    }
}
