//! One cache build: decode the file block by block straight into its sparse, pre-sized cache file (the mixer reads
//! each chunk as soon as it is written), write its loudness energies (and from them its loudness) and its peaks in
//! the same pass, then give them their real names. Memory stays at a few hundred KB whatever the file's length: nothing collects the whole decode.
//!
//! Each file is written through the durable path: written under a temporary name, synced (`Ordered`: on macOS a
//! barrier, ~ms, after the file's own write-back), then renamed, so a crash or power cut leaves either no cache or a
//! whole one, never a header over missing data. A cache reopened later is checked: one whose energies don't match it
//! (`Energies::open`) gets them made again from its decoded sound (`energy::rebuild`) while it already plays.
//!
//! The decode runs in stretches ("runs"): from the playhead to the end, then the gaps. A playhead that jumps far
//! from the run (`Progress::retarget`) ends it and starts the next run there; a run that reaches written chunks
//! stops. A closed project cancels the build (`Progress::cancel`), and so does its caller's own flag, checked beside it
//! at every block (`PcmStore::build_cancellable`). The pass reads sound only (sound first);
//! a video's thumbnails come from a scan after it.
//!
//! A read error part-way (a bad sector, an unplugged drive) is never taken for the end of the file. The runs that end
//! before the bad spot still decode (the start of the file before a playhead run that hit it), then the build fails
//! with where it is (`BuildError::Unreadable`), so the cache is never finished short. What decoded stays playable
//! (a file whose container states no length is mapped now, as decoded). The store tries such a file `READ_TRIES`
//! times per opening, then keeps that partial copy (silent from the bad spot), says so once, and waits for the next
//! opening (`PcmStore`).

use klipolo_platform::durable::{self, Durability};
use klipolo_platform::file::PositionedIo;
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use memmap2::Mmap;

use super::energy::{self, Energies, EnergyWriter};
use super::handle::CHUNK;
use super::peaks::Peaks;
use super::sink::Sink;
use super::tidy::touch;
use super::{BuildError, HEADER, Job, MAGIC, Pcm, PcmStore, lock};
use crate::audio::decode::{self, DecodeError};
use crate::audio::mix::RATE;
use crate::media::key::{CacheKind, stem};

pub(super) use super::sink::{Progress, RETARGET_AHEAD};

/// Numbers this process's temporary cache files (`<stem>.<pid>-<n>…tmp`, the names the tidy knows).
pub(super) static TMP: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    /// Tests: builds on this thread take their file's container as stating no length (a live recording's Matroska
    /// does), so they decode without a growing mapping.
    pub(super) static NO_STATED_LENGTH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn header(channels: usize, frames: u64, lufs: f64, complete: bool) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[..8].copy_from_slice(MAGIC);
    h[8..10].copy_from_slice(&(channels as u16).to_le_bytes());
    h[10..12].copy_from_slice(&1u16.to_le_bytes()); // s16 at half scale
    h[12..16].copy_from_slice(&RATE.to_le_bytes());
    h[16..24].copy_from_slice(&frames.to_le_bytes());
    h[24..28].copy_from_slice(&(lufs as f32).to_le_bytes());
    h[28..32].copy_from_slice(&u32::from(complete).to_le_bytes());
    h
}

/// Builds `job`'s file on this thread; `caller` cancels it like a close does (never set for the worker's builds).
pub(super) fn build(
    store: &PcmStore,
    job: &Job,
    progress: &Progress,
    caller: &AtomicBool,
) -> Result<Arc<Pcm>, BuildError> {
    let cancelled = || progress.cancel.load(Ordering::Acquire) || caller.load(Ordering::Acquire);
    let source = &job.key.path;
    // The file's size and modification time are in the name: an edited file gets a new cache.
    let stem = stem(source, &CacheKind::Pcm).ok_or_else(|| {
        BuildError::Source(match fs::metadata(source) {
            Err(e) => format!("The file can't be read: {e}"),
            Ok(_) => "The file can't be read.".into(),
        })
    })?;
    let (file_path, peaks_path, energy_path) = (
        store.dir.join(format!("{stem}.pcm")),
        store.dir.join(format!("{stem}.peaks")),
        store.dir.join(format!("{stem}.energy")),
    );
    {
        // Under the lock a close cancels in: a build it cancelled never records a file into the next opening.
        let mut inner = lock(&store.inner);
        if cancelled() {
            return Err(BuildError::Cancelled);
        }
        inner.used.insert(stem.clone(), source.clone());
    }
    if let Some(pcm) = Pcm::open(&file_path) {
        touch(&file_path);
        touch(&peaks_path);
        let pcm = Arc::new(pcm);
        store.publish(&mut lock(&store.inner), job.key.clone(), pcm.clone());
        // Its energies must match it. Missing, short or damaged ones (a crash or power cut mid-write, an older
        // format) are made again from the decoded sound, once, here on the build thread while the file already plays;
        // until then the preview measures exactly (they are never read as silence).
        if Energies::open(&energy_path, pcm.channels(), pcm.frames() as u64).is_some() {
            touch(&energy_path);
        } else {
            let tmp = store.dir.join(format!(
                "{stem}.{}-{}.energy.tmp",
                std::process::id(),
                TMP.fetch_add(1, Ordering::Relaxed)
            ));
            match energy::rebuild(&pcm, &energy_path, &tmp, &cancelled) {
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "audio energies couldn't be made again"),
            }
        }
        return Ok(pcm);
    }
    fs::create_dir_all(&store.dir)?;
    let probe = decode::probe(Path::new(source)).map_err(BuildError::Source)?;
    #[cfg(test)]
    let probe = decode::Probe {
        frames: probe.frames.filter(|_| !NO_STATED_LENGTH.get()),
        ..probe
    };
    let channels = usize::from(probe.channels);
    // A second of slack past the container's stated length (it is a little off at times).
    let capacity = probe.frames.map_or(0, |f| f + u64::from(RATE));
    let n = TMP.fetch_add(1, Ordering::Relaxed);
    let tag = format!("{stem}.{}-{n}", std::process::id());
    let tmp = store.dir.join(format!("{tag}.tmp"));
    let peaks_tmp = store.dir.join(format!("{tag}.peaks.tmp"));
    let energy_tmp = store.dir.join(format!("{tag}.energy.tmp"));
    let result = (|| {
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.set_len(HEADER as u64 + capacity * channels as u64 * 2)?;
        file.write_all_at(&header(channels, 0, f64::NAN, false), 0)?;
        // Playable while it decodes: the mixer reads the chunks marked written.
        let growing = (capacity > 0)
            // SAFETY: the mapping only reads chunks marked written, which happens after their bytes are.
            .then(|| unsafe { Mmap::map(&file) }.ok())
            .flatten()
            .map(|map| Arc::new(Pcm::mapped(map, channels, capacity, None)));
        if let Some(pcm) = &growing {
            store.publish(&mut lock(&store.inner), job.key.clone(), pcm.clone());
        }
        let energy = EnergyWriter::create(&energy_tmp, channels, capacity)?;
        let peaks = Peaks::create(&peaks_tmp, channels)?;
        let mut sink = Sink::new(
            &file,
            growing.as_deref(),
            progress,
            caller,
            channels,
            energy,
            peaks,
        );
        #[cfg(test)]
        {
            sink.hook = super::hooks::find(source);
        }
        let frames = match decode_runs(&mut sink, Path::new(source), job.start_at, capacity) {
            Ok(frames) => frames,
            // No length to lay chunks out on, so nothing was playable while it decoded: what decoded before the bad
            // spot is now (the store keeps it once reading has failed too often).
            Err(BuildError::Unreadable { message, at }) if growing.is_none() => {
                if let Some(pcm) = Pcm::decoded_so_far(&file, channels) {
                    store.publish(&mut lock(&store.inner), job.key.clone(), Arc::new(pcm));
                }
                return Err(BuildError::Unreadable { message, at });
            }
            Err(e) => return Err(e),
        };
        // A mono file counts on both speakers, as it plays.
        let lufs = sink.energy.finish(frames)?.unwrap_or(f64::NAN);
        sink.peaks.finish(frames)?;
        file.write_all_at(&header(channels, frames, lufs, true), 0)?;
        durable::sync(&file, Durability::Ordered)?;
        // The energies first: a cache file's name means its energies are there (and `Energies::open` checks them).
        fs::rename(&energy_tmp, &energy_path)?;
        fs::rename(&tmp, &file_path)?;
        let _ = fs::rename(&peaks_tmp, &peaks_path);
        // The renames themselves (Linux; macOS and Windows need nothing): lost ones only cost a rebuild.
        if let Err(error) = durable::fsync_dir(&store.dir) {
            tracing::warn!(%error, "the audio cache folder couldn't be synced");
        }
        match growing {
            Some(pcm) if frames <= capacity => {
                pcm.loudness.store(lufs.to_bits(), Ordering::Release);
                pcm.total.store(frames, Ordering::Release);
                pcm.complete.store(true, Ordering::Release);
                Ok(pcm)
            }
            // The container understated its length (or stated none): map the whole file again.
            _ => {
                let pcm = Arc::new(Pcm::open(&file_path).ok_or_else(|| {
                    BuildError::Cache(io::Error::other("The audio cache couldn't be read."))
                })?);
                store.publish(&mut lock(&store.inner), job.key.clone(), pcm.clone());
                Ok(pcm)
            }
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
        let _ = fs::remove_file(&peaks_tmp);
        let _ = fs::remove_file(&energy_tmp);
    }
    result
}

/// Decodes runs until every chunk up to the end is written; returns the file's length in frames. After a read error
/// part-way, the runs below the bad spot still decode; then it fails with the earliest one.
fn decode_runs(
    sink: &mut Sink,
    path: &Path,
    start_at: u64,
    capacity: u64,
) -> Result<u64, BuildError> {
    let Some(pcm) = sink.pcm else {
        // No length to lay chunks out on: one run, start to end.
        return sink.whole(path, 0);
    };
    let chunks = capacity.div_ceil(CHUNK);
    let mut next = start_at / CHUNK;
    let mut total: Option<u64> = None;
    // The earliest read error part-way: nothing from its chunk on is read again.
    let mut bad: Option<(u64, String)> = None;
    loop {
        if sink.cancelled() {
            return Err(BuildError::Cancelled);
        }
        let asked = sink.progress.retarget.swap(0, Ordering::AcqRel);
        if asked > 0 {
            next = (asked - 1) / CHUNK;
        }
        // Only mapped chunks are tracked; anything past the mapping is written by a run that goes there.
        let limit = total
            .map_or(chunks, |t| t.div_ceil(CHUNK))
            .min(chunks)
            .min(bad.as_ref().map_or(u64::MAX, |(at, _)| at / CHUNK));
        let missing = |from: u64| (from..limit).find(|&c| !pcm.chunk_ready(c));
        let start = match missing(next.min(limit)).or_else(|| missing(0)) {
            Some(c) => c,
            None => match bad {
                Some((at, message)) => return Err(BuildError::Unreadable { message, at }),
                None if total.is_some() => break,
                // Every mapped chunk is written but the end wasn't reached: decode on past the mapping.
                None => chunks,
            },
        };
        sink.begin(start * CHUNK, true)?;
        match decode::decode(path, start * CHUNK, None, &mut |at, s| sink.push(at, s)) {
            Ok(()) if sink.stopped => {
                sink.flush()?;
                sink.peaks.end_run(false)?;
                sink.energy.end_run(false)?;
                next = start;
            }
            Ok(()) => {
                total = Some(sink.reached_end()?);
                next = 0;
            }
            // The file can't seek there: decode it in order, start to end, once.
            Err(DecodeError::CantSeek) if start > 0 => return sink.whole(path, 0),
            Err(e) => match sink.failed(e) {
                // What decoded before it is written, its chunk included (so a run from below stops there); the
                // runs below it still decode.
                BuildError::Unreadable { message, at } => {
                    sink.peaks.end_run(false)?;
                    sink.energy.end_run(false)?;
                    if bad.as_ref().is_none_or(|(b, _)| at < *b) {
                        bad = Some((at, message));
                    }
                    next = 0;
                }
                error => return Err(error),
            },
        }
    }
    sink.flush()?;
    Ok(total.unwrap_or(sink.end))
}
