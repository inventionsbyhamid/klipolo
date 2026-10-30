//! One screen file's pass: read what is on disk, decode only the chunks it misses, push their pictures through
//! `Differ`, and append each chunk durably as it is made.
//!
//! **The file** (the contract, `klipolo_cursor::activity::file`): chunks append to `<stem>.activity.part`, each followed
//! by an ordered sync, so a stop (a cancel, a hold, a crash, a power cut) loses at most the chunk being made. The part
//! file is decoded with the screen file's own duration (the decoder's, the one its header stores), truncated to
//! `append_at` (a new header when that is 0: another duration, or a damaged header), and the chunks `missing` lists are
//! appended. Once every chunk is made it is renamed to `<stem>.activity`. A finished file that reads with a damaged
//! chunk is moved back to the part file, and only that chunk is made again (the later copy is read).
//!
//! **One writer.** The part file is locked while a pass writes it (`lock_exclusive`), so an export's foreground pass
//! (another process) and the editor's never append to it together: the second one is `Busy` and comes back.
//!
//! **Ticks.** Tick `i` is the change between the pictures at `i` and `i + 1` × 100 ms, so chunk `c` reads pictures
//! `600c` to `600c + 600`: the differ is primed with the chunk's first picture, which is the last one of the chunk
//! before when that was just made. A chunk holds fewer ticks only where the video ends.
//!
//! **A minute it can't read** (a picture fails: damaged data) is passed over: the decoder goes, the chunks after it are
//! made from a decoder opened at their own start, and it stays missing for a later pass (`Partial`). A decoder that
//! m5). A decoder that won't open again there (a drive gone) ends the pass at once, `Unreadable`.
//! A file marked as one that can't be analysed (`files::failed_mark`) isn't opened.
//!
//! **Cost:** per tick one picture and one `Differ::push` (≈ 10 µs); per chunk an 8.4 KB append, a sync, and the
//! track read again from the bytes in memory (≈ 1 ms for two hours) for the store. **Memory:** the file's bytes
//! (≤ 0.5 MB an hour) and one decoded track, besides the decoder.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use klipolo_cursor::activity::{
    ActivityTrack, CHUNK_TICKS, Differ, MAX_DURATION_US, Tick, decode, encode_chunk, header,
    missing, stored_duration,
};
use klipolo_model::time::Us;
use klipolo_platform::durable::{self, DiskReason, Durability, classify};

use super::files::{CHUNK_US, chunks_of, failed_mark, files, finished};
use crate::platform::activity::{ActivityError, ActivityFrames, LUMA_BYTES, Opener};

mod pictures;

use pictures::{Chunk, Decoding};

/// What the pass may do before its next picture.
pub(crate) enum Pace {
    Go,
    /// Something holds background work longer than the grace: stop, keeping every chunk made.
    Held,
    /// Cancelled.
    Stop,
}

/// Why a pass ends `Failed` on a file marked as one that can't be analysed.
pub(crate) const MARKED: &str = "marked as a file that can't be analysed";

/// How a pass left its file.
pub(crate) enum Ended {
    Complete(Arc<ActivityTrack>),
    /// Every chunk it could read is made (the track); the others couldn't be read (the first one's error), and a later
    /// pass makes them.
    Partial(Arc<ActivityTrack>, String),
    /// Cancelled, or held: every chunk made is kept.
    Stopped,
    Held,
    /// Another pass (another process) is writing it.
    Busy,
    /// The file isn't there now (an unplugged drive), or reading it failed part-way: tried again later.
    Unreadable(String),
    /// It can't be analysed, for good (no video, no length, a codec nobody decodes, no picture at 0, or marked so):
    /// only this is ever marked (`ActivityStore::fail`).
    Failed(String),
    DiskFull,
}

/// One file's pass.
pub(crate) struct Pass<'a> {
    pub root: &'a Path,
    pub path: &'a str,
    pub stem: &'a str,
    pub opener: &'a Opener,
    /// The decoder stops between packets once it is set.
    pub cancel: &'a Arc<AtomicBool>,
    pub pace: &'a dyn Fn() -> Pace,
    /// After each chunk: the track so far and the share of its chunks made.
    pub made: &'a dyn Fn(Arc<ActivityTrack>, f32),
}

impl From<io::Error> for Ended {
    fn from(e: io::Error) -> Self {
        match classify(&e) {
            DiskReason::Full => Ended::DiskFull,
            _ => Ended::Unreadable(e.to_string()),
        }
    }
}

impl From<ActivityError> for Ended {
    fn from(e: ActivityError) -> Self {
        match e {
            ActivityError::Cancelled => Ended::Stopped,
            ActivityError::Unsupported(e) => Ended::Failed(e),
            ActivityError::Unavailable(e) => Ended::Unreadable(e),
            ActivityError::Decode(e) => Ended::Unreadable(e),
            ActivityError::DiskFull => Ended::DiskFull,
        }
    }
}

impl Pass<'_> {
    pub(crate) fn run(&self) -> Ended {
        match self.write() {
            Ok(ended) | Err(ended) => ended,
        }
    }

    fn open(&self, from_us: Us) -> Result<Box<dyn ActivityFrames>, Ended> {
        (self.opener)(Path::new(self.path), from_us, self.cancel).map_err(Ended::from)
    }

    fn write(&self) -> Result<Ended, Ended> {
        let (done, part) = files(self.root, self.stem);
        if failed_mark(self.root, self.stem).exists() {
            return Ok(Ended::Failed(MARKED.into()));
        }
        if done.exists() {
            match finished(&done) {
                Some(track) if track.is_complete() => return Ok(Ended::Complete(Arc::new(track))),
                // A damaged chunk (appended again), a header it can't read (written again from the start) or a read
                // error (which comes back below as `Unreadable`, tried `READ_TRIES` times): the part file takes it.
                _ => std::fs::rename(&done, &part)?,
            }
        }
        std::fs::create_dir_all(self.root)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part)?;
        match klipolo_platform::process::lock_exclusive(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(Ended::Busy),
            Err(e) => return Err(e.into()),
        }
        if done.exists() {
            // Another pass finished it since: its track is read next time.
            return Ok(Ended::Busy);
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // Opened where the stored duration says the work starts; checked against the decoder's own.
        let stored = stored_duration(&bytes).map(|d| decode(&bytes, d));
        let first = stored.as_ref().and_then(|t| missing(t).first().copied());
        let from = first.map_or(0, |c| c as Us * CHUNK_US);
        let frames = self.open(from)?;
        let duration = frames.duration_us().clamp(0, MAX_DURATION_US);
        let track = match stored {
            Some(t) if t.duration_us() == duration => t,
            _ => decode(&bytes, duration),
        };
        bytes.truncate(track.append_at());
        if bytes.is_empty() {
            header(duration, &mut bytes);
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&bytes)?;
        } else {
            file.set_len(bytes.len() as u64)?;
            file.seek(SeekFrom::End(0))?;
        }
        let todo = missing(&track);
        drop(track);
        let chunks = chunks_of(duration).max(1) as f32;
        let mut chunk_bytes = Vec::with_capacity(6 + CHUNK_TICKS as usize * 14 + 4);
        let mut ticks: Vec<Tick> = Vec::with_capacity(CHUNK_TICKS as usize);
        let mut d = Decoding {
            frames: Some(frames),
            at: from,
            differ: Differ::new(),
            luma: Box::new([0u8; LUMA_BYTES]),
            primed: None,
            ended: false,
        };
        // The first minute it couldn't read, how many it passed over, and how many chunks it has made or passed over.
        let (mut skipped, mut lost, mut gone) = (None, 0usize, 0usize);
        for &chunk in &todo {
            gone += 1;
            if let Chunk::Lost(error) = self.ticks(&mut d, chunk, duration, &mut ticks)? {
                if skipped.is_none() {
                    tracing::warn!(chunk, %error, file = %crate::log_name(self.path), "screen activity: a minute couldn't be read; passed over");
                }
                skipped.get_or_insert(error);
                lost += 1;
                d.frames = None;
                d.primed = None;
                continue;
            }
            chunk_bytes.clear();
            encode_chunk(chunk, &ticks, &mut chunk_bytes);
            file.write_all(&chunk_bytes)?;
            durable::sync(&file, Durability::Ordered)?;
            bytes.extend_from_slice(&chunk_bytes);
            let track = Arc::new(decode(&bytes, duration));
            let share = (chunks - (todo.len() - gone) as f32) / chunks;
            (self.made)(track, share.clamp(0.0, 1.0));
        }
        drop(d);
        if lost > 1 {
            tracing::warn!(lost, file = %crate::log_name(self.path), "screen activity: minutes passed over");
        }
        match skipped {
            Some(error) => Ok(Ended::Partial(Arc::new(decode(&bytes, duration)), error)),
            None => self.finish(&file, &part, &done, &bytes, duration),
        }
    }

    /// Every chunk made: the part file becomes the finished one (its rename made durable).
    fn finish(
        &self,
        file: &File,
        part: &Path,
        done: &Path,
        bytes: &[u8],
        duration: Us,
    ) -> Result<Ended, Ended> {
        let track = decode(bytes, duration);
        if !track.is_complete() {
            // Never meant to happen, and never a reason to mark the file: the next pass makes what's missing.
            return Ok(Ended::Unreadable(
                "chunks are still missing after a pass".into(),
            ));
        }
        durable::sync(file, Durability::Ordered)?;
        std::fs::rename(part, done)?;
        durable::fsync_dir(self.root)?;
        Ok(Ended::Complete(Arc::new(track)))
    }
}
