//! Builds that jump to a new playhead, builds a closed project cancels, and which cache files a close keeps.

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use super::build::Progress;
use super::{BuildError, CHUNK, Job, PcmKey, PcmStore, lock, tidy};
use crate::audio::mix::RATE;

fn value(n: u64) -> f32 {
    (0.5 * (2.0 * std::f64::consts::PI * 440.0 * n as f64 / f64::from(RATE)).sin()) as f32
}

/// A 16-bit mono WAV of `value`.
pub(super) fn write_wav(path: &Path, seconds: u64) {
    let frames = seconds * u64::from(RATE);
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let data = (frames * 2) as u32;
    for part in [
        &b"RIFF"[..],
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &RATE.to_le_bytes(),
        &(RATE * 2).to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ] {
        out.write_all(part).unwrap();
    }
    for n in 0..frames {
        out.write_all(&((value(n) * 32767.0).round() as i16).to_le_bytes())
            .unwrap();
    }
}

/// A build of `seconds` of WAV registered as running, with its progress.
fn running(dir: &Path, seconds: u64) -> (Arc<PcmStore>, Job, Arc<Progress>) {
    let wav = dir.join("long.wav");
    write_wav(&wav, seconds);
    let store = PcmStore::new(dir.join("cache"));
    let key = PcmKey {
        path: wav.display().to_string(),
    };
    let progress = Arc::new(Progress::default());
    lock(&store.inner)
        .building
        .insert(key.clone(), progress.clone());
    (store, Job { key, start_at: 0 }, progress)
}

#[test]
fn a_running_build_jumps_to_a_far_playhead_then_fills_in_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let (store, job, progress) = running(dir.path(), 60);
    // The playhead jumps to 45 s as the build starts from 0.
    let target = 45 * u64::from(RATE);
    progress.retarget.store(target + 1, Ordering::Release);
    let builder = {
        let (store, job, progress) = (store.clone(), job.clone(), progress.clone());
        std::thread::spawn(move || store.run_job(&job, progress))
    };
    let early = 20 * u64::from(RATE);
    let (mut at_target, mut at_early) = (None, None);
    let started = Instant::now();
    while at_early.is_none() && started.elapsed().as_secs() < 60 {
        if let Some(pcm) = store.ready(&job.key) {
            if at_target.is_none() && pcm.is_ready(target) {
                at_target = Some(started.elapsed());
            }
            if pcm.is_ready(early) {
                at_early = Some(started.elapsed());
            }
        }
        std::thread::yield_now();
    }
    let pcm = builder.join().unwrap().unwrap();
    assert!(
        at_target < at_early,
        "45 s at {at_target:?}, 20 s at {at_early:?}"
    );
    // The runs join up exactly.
    let join = target / CHUNK * CHUNK;
    let mut one = [0f32];
    for n in [0, early, join - 1, join, join + 1, 59 * u64::from(RATE)] {
        pcm.read(n as i64, &mut one);
        assert!(
            (one[0] - value(n)).abs() < 0.001,
            "frame {n}: {} vs {}",
            one[0],
            value(n)
        );
    }
    assert_eq!(pcm.frames(), 60 * RATE as usize);
    // The energies come out as one straight run writes them, however the runs went: each chunk once, the windows
    // across chunk edges whole.
    let straight = PcmStore::new(dir.path().join("straight"));
    let straight_pcm = straight.build(&job.key).unwrap();
    let (a, b) = (
        store.energies(&job.key, &pcm).unwrap(),
        straight.energies(&job.key, &straight_pcm).unwrap(),
    );
    assert_eq!(a.frames(), 60 * u64::from(RATE));
    for w in 0..600 {
        // Runs start their weighting at rest, which settles within a few ms of a run's first window.
        let (x, y) = (a.power(w), b.power(w));
        assert!((x - y).abs() <= 0.02 * y, "window {w}: {x} vs {y}");
    }
    // The file's own loudness comes from its energies: what libebur128 measures of it, played on both speakers.
    let mut meter = ebur128::EbuR128::new(1, RATE, ebur128::Mode::I).unwrap();
    meter.set_channel(0, ebur128::Channel::DualMono).unwrap();
    let samples: Vec<f32> = (0..60 * u64::from(RATE)).map(value).collect();
    meter.add_frames_f32(&samples).unwrap();
    let exact = meter.loudness_global().unwrap();
    let lufs = pcm.loudness().unwrap();
    assert!((lufs - exact).abs() < 0.02, "{lufs} vs {exact}");
}

#[test]
fn a_cancelled_build_leaves_nothing_and_can_run_again() {
    let dir = tempfile::tempdir().unwrap();
    let (store, job, progress) = running(dir.path(), 5);
    progress.cancel.store(true, Ordering::Release);
    assert!(matches!(
        store.run_job(&job, progress),
        Err(BuildError::Cancelled)
    ));
    let left: Vec<_> = std::fs::read_dir(dir.path().join("cache"))
        .map(|dir| dir.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    let inner = lock(&store.inner);
    assert!(!inner.failed.contains_key(&job.key) && !inner.ready.contains_key(&job.key));
    assert!(inner.used.is_empty(), "a cancelled build records no file");
}

/// The cache folder's file names, sorted.
fn cached(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_close_deletes_nothing_and_the_tidy_evicts_the_oldest_unused_file_first() {
    let dir = tempfile::tempdir().unwrap();
    let key = |name: &str| {
        let wav = dir.path().join(name);
        write_wav(&wav, 1);
        PcmKey {
            path: wav.display().to_string(),
        }
    };
    let (old, dropped, kept) = (key("old.wav"), key("dropped.wav"), key("kept.wav"));
    let store = PcmStore::new(dir.path().join("cache"));
    let cache = dir.path().join("cache");
    // A project used all three, then dropped one before closing: every cache survives the close.
    for k in [&old, &dropped, &kept] {
        store.build(k).unwrap();
    }
    store.close_project();
    assert_eq!(cached(&cache).len(), 9, "{:?}", cached(&cache));
    // Oldest first: "old", then "dropped", then "kept" (modification time is the LRU order).
    let stem_of =
        |k: &PcmKey| crate::media::key::stem(&k.path, &crate::media::key::CacheKind::Pcm).unwrap();
    for (i, k) in [&old, &dropped, &kept].into_iter().enumerate() {
        for ext in ["pcm", "peaks", "energy"] {
            let at = SystemTime::now() - Duration::from_secs(3600 * (3 - i as u64));
            std::fs::File::options()
                .write(true)
                .open(cache.join(format!("{}.{ext}", stem_of(k))))
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
    }
    let one = std::fs::metadata(cache.join(format!("{}.pcm", stem_of(&old))))
        .unwrap()
        .len()
        + std::fs::metadata(cache.join(format!("{}.peaks", stem_of(&old))))
            .unwrap()
            .len()
        + std::fs::metadata(cache.join(format!("{}.energy", stem_of(&old))))
            .unwrap()
            .len();
    // Over the budget by one file's worth, with "old" in use by the open project: "dropped" goes, "old" stays.
    tidy::tidy(&cache, false, 2 * one, || HashSet::from([stem_of(&old)]));
    let left = cached(&cache).join(" ");
    assert!(
        left.contains(&stem_of(&old)) && left.contains(&stem_of(&kept)),
        "{left}"
    );
    assert!(!left.contains(&stem_of(&dropped)), "{left}");
}

#[test]
fn a_build_a_close_cancelled_never_takes_the_place_of_the_next_ones() {
    let dir = tempfile::tempdir().unwrap();
    let (store, job, progress) = running(dir.path(), 5);
    store.close_project();
    assert!(progress.cancel.load(Ordering::Acquire));
    // The next project may queue the same file at once (it isn't "building" any more)…
    assert!(!lock(&store.inner).building.contains_key(&job.key));
    let next = Arc::new(Progress::default());
    lock(&store.inner)
        .building
        .insert(job.key.clone(), next.clone());
    // …and the cancelled build winding down leaves that build in place.
    assert!(matches!(
        store.run_job(&job, progress),
        Err(BuildError::Cancelled)
    ));
    let inner = lock(&store.inner);
    assert!(
        inner
            .building
            .get(&job.key)
            .is_some_and(|p| Arc::ptr_eq(p, &next))
    );
}

#[test]
fn startup_tidy_keeps_unfinished_files_a_build_may_still_be_writing() {
    let dir = tempfile::tempdir().unwrap();
    let hour_ago = SystemTime::now() - Duration::from_secs(3600);
    let make = |name: String, modified: SystemTime| {
        let path = dir.path().join(name);
        std::fs::File::create(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        path
    };
    // This process is alive (as an export worker would be); 999999999 is no process.
    let live = make(format!("aaaa.{}-0.tmp", std::process::id()), hour_ago);
    let recent = make("bbbb.999999999-0.peaks.tmp".into(), SystemTime::now());
    let stale = make("cccc.999999999-1.tmp".into(), hour_ago);
    tidy::tidy(dir.path(), true, super::DISK_BUDGET, HashSet::new);
    assert!(live.exists() && recent.exists());
    assert!(!stale.exists());
}

/// A drive unplugged mid-build: reading fails part-way (here a connection reset: an I/O error, as from a gone
/// drive). The decode fails rather than ending there as if the file did, so the build never finishes a short cache
/// that looks complete; it is tried again later (`RETRY_AFTER`).
#[cfg(unix)]
#[test]
fn a_read_error_part_way_fails_the_decode_instead_of_ending_it() {
    use std::net::TcpListener;
    use std::os::fd::AsRawFd;

    use crate::audio::decode::{DecodeError, decode_input};

    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("long.wav");
    write_wav(&wav, 20);
    let bytes = std::fs::read(&wav).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("tcp://{}", listener.local_addr().unwrap());
    let (decoding, started) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        // The header and 8 s of the 20 s it announces (opening reads about 5 s ahead); once the decode is under way
        // the connection is reset.
        let _ = stream.write_all(&bytes[..44 + 8 * RATE as usize * 2]);
        let _ = started.recv_timeout(Duration::from_secs(30));
        let reset = libc::linger {
            l_onoff: 1,
            l_linger: 0,
        };
        // SAFETY: a plain socket option on the stream's own descriptor, from a value that outlives the call.
        unsafe {
            libc::setsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                (&raw const reset).cast(),
                size_of::<libc::linger>() as libc::socklen_t,
            )
        };
    });
    let mut frames = 0u64;
    // Opened directly: `media::open` (every media open's) reads files only, never a connection.
    let _ = ffmpeg::init();
    let input = ffmpeg::format::input(&url).unwrap();
    let decoded = decode_input(input, 0, None, &mut |_, block| {
        if frames == 0 {
            let _ = decoding.send(());
        }
        frames += block.len() as u64;
        Ok(true)
    });
    server.join().unwrap();
    assert!(
        matches!(decoded, Err(DecodeError::Unreadable(_))),
        "{decoded:?} after {frames} frames"
    );
    // It ends where the data stopped, 12 s before the end the file announces.
    assert!(frames > 0 && frames <= 8 * u64::from(RATE), "{frames}");
}

/// `tone-440.aac` (4 s of a mono 48 kHz tone, ADTS) whose reading fails 2 s in: 2000 frame headers that announce no
/// data, each a damaged packet to the demuxer (a long run of them reads as a bad stretch of the file). Returns its path
/// and where the damage starts, in frames.
fn damaged_tone(dir: &Path) -> (std::path::PathBuf, u64) {
    let fixture = crate::fixture_set::fixture_dir().join("tone-440.aac");
    let src = std::fs::read(&fixture).expect("run scripts/gen-engine-fixtures.sh");
    // ADTS frames of 1024 samples each, up to the one that starts at 2 s.
    let (mut at, mut frames) = (0usize, 0u64);
    while frames * 1024 < 2 * u64::from(RATE) {
        let h = &src[at..at + 7];
        at += (usize::from(h[3] & 3) << 11) | (usize::from(h[4]) << 3) | (usize::from(h[5]) >> 5);
        frames += 1;
    }
    let mut empty = src[at..at + 7].to_vec();
    (empty[3], empty[4], empty[5]) = (empty[3] & !3, 0, empty[5] & 0x1F);
    let mut out = src[..at].to_vec();
    (0..2000).for_each(|_| out.extend_from_slice(&empty));
    out.extend_from_slice(&src[at..]);
    let path = dir.join("damaged.aac");
    std::fs::write(&path, out).unwrap();
    (path, frames * 1024)
}

/// A file whose reading keeps failing part-way is built at most
/// `READ_TRIES` times per opening. Then what decoded stays playable up to the bad spot and is silent after it, the
/// user hears about it once, and it isn't built again until the project is opened again. Whether the build starts at
/// the file's start or at a playhead before the bad spot (the start is decoded after the playhead's run fails), and
/// when the container states no length (nothing is mapped while it decodes).
#[test]
fn a_file_whose_reading_keeps_failing_plays_up_to_there_and_waits_for_the_next_opening() {
    use std::sync::Mutex;

    use super::build::NO_STATED_LENGTH;
    use crate::audio::plan::{AudioPlan, AudioSource};
    use crate::media::events::MediaEvent;
    use crate::media::store::READ_TRIES;
    use klipolo_model::project::Loudness;

    /// Builds on this thread read containers' lengths again, even if a case fails.
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            NO_STATED_LENGTH.set(false);
        }
    }
    let _reset = Reset;
    for (start_at, no_length) in [(0, false), (CHUNK, false), (0, true)] {
        let case = format!("start {start_at}, no stated length {no_length}");
        NO_STATED_LENGTH.set(no_length);
        let dir = tempfile::tempdir().unwrap();
        let (path, bad) = damaged_tone(dir.path());
        let store = PcmStore::new(dir.path().join("cache"));
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        store.set_sink(Arc::new(move |e| seen.lock().unwrap().push(e)));
        let key = PcmKey {
            path: path.display().to_string(),
        };
        let plan = AudioPlan {
            sources: vec![AudioSource {
                path: key.path.clone(),
                start_us: 0,
                end_us: 4_000_000,
                source_in_us: 0,
                speed: 1.0,
                rate: 1.0,
                gain: 1.0,
                fade_in_us: 0,
                fade_out_us: 0,
                crossfade: Default::default(),
                filter: None,
                centre_from: None,
                duck: None,
            }],
            loudness: Loudness::Off,
        };
        let job = Job {
            key: key.clone(),
            start_at,
        };
        for n in 1..=READ_TRIES {
            let progress = Arc::new(Progress::default());
            lock(&store.inner)
                .building
                .insert(key.clone(), progress.clone());
            let built = store.run_job(&job, progress);
            let Err(BuildError::Unreadable { at, .. }) = built else {
                panic!("{case}, build {n}: {built:?}", built = built.map(|_| ()));
            };
            // The AAC decoder's priming moves what it hands out by about a frame.
            assert!(
                at.abs_diff(bad) <= 2048,
                "{case}: stopped at {at}, damaged at {bad}"
            );
            let inner = lock(&store.inner);
            assert_eq!(
                inner.damaged.contains(&key),
                n == READ_TRIES,
                "{case}, build {n}"
            );
        }
        // Plays up to the bad spot (its start too), silent after it.
        let pcm = store.ready(&key).expect("the part that decoded");
        assert!(!pcm.is_superseded() && !pcm.is_complete(), "{case}");
        let rms = |at: u64| {
            let mut out = vec![0f32; 4800];
            pcm.read(at as i64, &mut out);
            (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt()
        };
        for at in [RATE / 2, RATE] {
            assert!(rms(u64::from(at)) > 0.01, "{case}: {at} frames play");
        }
        assert_eq!(rms(3 * u64::from(RATE)), 0.0, "{case}: 3 s is silent");
        // Held: no prepare builds it again (every play, seek or edit asks), even once the retry timer has run out
        // and with no copy kept.
        {
            let mut inner = lock(&store.inner);
            inner.failed.clear();
            inner.ready.remove(&key);
        }
        store.prepare(&plan, 0);
        assert!(
            lock(&store.inner).queue.is_empty(),
            "{case}: not built again"
        );
        // One notice, where the sound stops.
        let notices: Vec<_> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                MediaEvent::AudioDamaged { path, at_us } => Some((path.clone(), *at_us)),
                MediaEvent::Changed { .. } => None,
            })
            .collect();
        assert_eq!(notices.len(), 1, "{case}: {notices:?}");
        assert_eq!(notices[0].0, key.path);
        assert!(
            (notices[0].1 - 2_000_000).abs() < 50_000,
            "{case}: {notices:?}"
        );
        // The next opening tries it again.
        store.close_project();
        let inner = lock(&store.inner);
        assert!(
            inner.damaged.is_empty() && inner.unreadable.is_empty(),
            "{case}"
        );
    }
}

/// A cache's energies are used only when they match it. A truncated file (a crash mid-write), a
/// valid header over zeroed windows (a power cut that kept the header but not the data) and an older format are each
/// refused, never read as silence, and the next opening of the cache makes them again from its decoded sound, without
/// decoding the file again.
#[test]
fn damaged_energies_are_refused_and_made_again_from_the_cached_sound() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("voice.wav");
    write_wav(&wav, 5);
    let key = PcmKey {
        path: wav.display().to_string(),
    };
    let cache = dir.path().join("cache");
    let first = PcmStore::new(cache.clone());
    let pcm = first.build(&key).unwrap();
    let original: Vec<f64> = {
        let good = first.energies(&key, &pcm).unwrap();
        (0..50).map(|w| good.power(w)).collect()
    };
    let stem = crate::media::key::stem(&key.path, &crate::media::key::CacheKind::Pcm).unwrap();
    let energy = cache.join(format!("{stem}.energy"));
    let bytes = std::fs::read(&energy).unwrap();
    // The cached sound's file number: decoding again would write a new file (checked where files have numbers).
    #[cfg(unix)]
    let (sound, decoded) = {
        let sound = cache.join(format!("{stem}.pcm"));
        let number = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&sound).unwrap());
        (sound, number)
    };
    let zeroed = {
        let mut b = bytes.clone();
        b[32..].fill(0);
        b
    };
    let older = {
        let mut b = bytes.clone();
        b[10..12].fill(0);
        b
    };
    for (what, damaged) in [
        ("truncated", bytes[..bytes.len() / 2].to_vec()),
        ("zeroed windows", zeroed),
        ("an older format", older),
    ] {
        std::fs::write(&energy, &damaged).unwrap();
        assert!(first.energies(&key, &pcm).is_none(), "{what}: refused");
        let next = PcmStore::new(cache.clone());
        let reopened = next.build(&key).unwrap();
        let made = next.energies(&key, &reopened).expect(what);
        for (w, was) in original.iter().enumerate() {
            let now = made.power(w);
            assert!(
                (now - was).abs() <= 1e-3 * was,
                "{what}, window {w}: {now} vs {was}"
            );
        }
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&sound).unwrap()),
            decoded,
            "{what}: the sound wasn't decoded again"
        );
    }
    assert!(cached(&cache).iter().all(|n| !n.ends_with(".tmp")));
}
