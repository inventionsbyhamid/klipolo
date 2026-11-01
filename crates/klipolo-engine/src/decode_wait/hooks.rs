//! Test hooks (tests and `trial` builds only, like `render::pool::slow_stats`): a file whose AVAssetReaders stall before
//! a frame, and a file whose FFmpeg decoders are slow, so the stalled-decoder tests and the trial smoke can reproduce
//! an export that failed under load without loading the computer. They also record what the decoders did (the first
//! frame each reader vended, the frames software decoded). Blocked threads wake when the hooks are cleared or set again.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::DecodeWait;

/// How long a stalled reader blocks.
#[derive(Clone, Copy, Debug)]
pub enum Stall {
    For(Duration),
    /// Until the hook is cleared (a reader that never returns).
    Forever,
}

/// Readers that block once, before vending `at_frame` or a later frame (`stall_reads`).
#[derive(Clone, Debug)]
pub struct ReadStall {
    /// Which readers made on the file from now (0 is the first).
    pub readers: Range<usize>,
    pub at_frame: u32,
    pub stall: Stall,
}

/// FFmpeg decoders that are slowed (`slow_software`).
#[derive(Clone, Debug)]
pub struct SlowSoftware {
    /// Which decoders opened on the file from now (0 is the first).
    pub decoders: Range<usize>,
    /// Frames this far into the file or later are slowed.
    pub from_s: f64,
    /// How much longer each of them takes (a long time: a decoder that stops).
    pub per_frame: Duration,
    /// How many of them, per decoder.
    pub frames: u32,
}

#[derive(Default)]
struct FileHooks {
    /// Bumped when the hooks change: a thread blocked under older settings goes on.
    generation: u64,
    reads: Vec<ReadStall>,
    /// (rule, reader) pairs that have stalled already: each reader stalls once per rule.
    stalled: HashSet<(usize, usize)>,
    slow: Vec<SlowSoftware>,
    /// Per reader made since the hooks were set, in order: the first frame it vended.
    readers: Vec<Option<u32>>,
    /// FFmpeg decoders opened since the hooks were set.
    software: usize,
    /// Frames slowed so far, by (rule, decoder).
    slowed: HashMap<(usize, usize), u32>,
    /// Each frame FFmpeg decoded, in order: its decoder's number, and when (seconds into the file) it is.
    decoded: Vec<(usize, f64)>,
}

static HOOKS: Mutex<Option<HashMap<Arc<str>, FileHooks>>> = Mutex::new(None);
static EXPORT: Mutex<Option<DecodeWait>> = Mutex::new(None);
static REOPEN_QUIET: Mutex<Option<Duration>> = Mutex::new(None);

fn hooks() -> MutexGuard<'static, Option<HashMap<Arc<str>, FileHooks>>> {
    HOOKS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn with<T>(path: &str, f: impl FnOnce(&mut FileHooks) -> T) -> Option<T> {
    hooks().as_mut()?.get_mut(path).map(f)
}

fn set(path: Arc<str>, f: impl FnOnce(&mut FileHooks)) {
    let mut all = hooks();
    let hooks = all
        .get_or_insert_with(HashMap::new)
        .entry(path)
        .or_default();
    hooks.generation += 1;
    f(hooks);
}

/// AVAssetReaders made on `path` from now stall as `rules` say (each reader once per rule it matches).
pub fn stall_reads(path: impl Into<Arc<str>>, rules: impl IntoIterator<Item = ReadStall>) {
    let rules = rules.into_iter().collect();
    set(path.into(), |h| {
        h.reads = rules;
        h.stalled.clear();
        h.readers.clear();
    });
}

/// FFmpeg decoders opened on `path` from now are slowed as `rules` say (the first rule that matches a frame).
pub fn slow_software(path: impl Into<Arc<str>>, rules: impl IntoIterator<Item = SlowSoftware>) {
    let rules = rules.into_iter().collect();
    set(path.into(), |h| {
        h.slow = rules;
        h.software = 0;
        h.slowed.clear();
        h.decoded.clear();
    });
}

/// Removes `path`'s hooks; threads they block go on.
pub fn clear(path: &str) {
    if let Some(all) = hooks().as_mut() {
        all.remove(path);
    }
}

/// The first frame each reader made on `path` since its hooks were set vended (None: none yet), in order.
pub fn readers_made(path: &str) -> Vec<Option<u32>> {
    with(path, |h| h.readers.clone()).unwrap_or_default()
}

/// Each frame an FFmpeg decoder of `path` decoded since its hooks were set, in order: the decoder's number (0 is the
/// first opened since), and when (seconds into the file) the frame is.
pub fn software_decoded(path: &str) -> Vec<(usize, f64)> {
    with(path, |h| h.decoded.clone()).unwrap_or_default()
}

/// Export's wait, for tests that need shorter windows (None: `render::EXPORT_TUNING`'s again).
pub fn set_export_wait(wait: Option<DecodeWait>) {
    *EXPORT.lock().unwrap_or_else(PoisonError::into_inner) = wait;
}

/// A test's export wait (`set_export_wait`), if any.
pub fn export_wait() -> Option<DecodeWait> {
    *EXPORT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long the preview's reopened reader must go without a stall before it may be reopened again, for tests that
/// can't wait two minutes (None: the pool's own).
pub fn set_reopen_quiet(quiet: Option<Duration>) {
    *REOPEN_QUIET.lock().unwrap_or_else(PoisonError::into_inner) = quiet;
}

/// A test's quiet period (`set_reopen_quiet`), if any.
pub fn reopen_quiet() -> Option<Duration> {
    *REOPEN_QUIET.lock().unwrap_or_else(PoisonError::into_inner)
}

fn key(path: &Path) -> std::borrow::Cow<'_, str> {
    path.to_string_lossy()
}

/// A reader was made on `path`: its number among those since the hooks were set (None without hooks).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn reader_made(path: &Path) -> Option<usize> {
    with(&key(path), |h| {
        h.readers.push(None);
        h.readers.len() - 1
    })
}

/// Reader `reader` of `path` is about to vend `frame`: recorded, and blocked if a stall applies to it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn vending(path: &Path, reader: Option<usize>, frame: u32) {
    let Some(reader) = reader else {
        return;
    };
    let key = key(path);
    let block = with(&key, |h| {
        if let Some(first) = h.readers.get_mut(reader)
            && first.is_none()
        {
            *first = Some(frame);
        }
        let (rule, stall) = h.reads.iter().enumerate().find_map(|(i, r)| {
            (r.readers.contains(&reader) && frame >= r.at_frame).then_some((i, r.stall))
        })?;
        h.stalled
            .insert((rule, reader))
            .then_some((stall, h.generation))
    })
    .flatten();
    if let Some((stall, generation)) = block {
        tracing::info!(file = %crate::log_name(path), frame, reader, ?stall, "test hook: the reader stalls");
        let until = match stall {
            Stall::For(d) => Some(Instant::now() + d),
            Stall::Forever => None,
        };
        wait(&key, generation, until);
    }
}

/// An FFmpeg decoder was opened on `path`: its number among those since the hooks were set (None without hooks).
pub(crate) fn software_opened(path: &Path) -> Option<usize> {
    with(&key(path), |h| {
        h.software += 1;
        h.software - 1
    })
}

/// FFmpeg decoder `decoder` of `path` decoded the frame `secs` into the file: recorded, and slowed if a rule says.
pub(crate) fn software_frame(path: &Path, decoder: Option<usize>, secs: f64) {
    let Some(decoder) = decoder else {
        return;
    };
    let key = key(path);
    let slow = with(&key, |h| {
        h.decoded.push((decoder, secs));
        let (rule, per_frame, frames) = h.slow.iter().enumerate().find_map(|(i, s)| {
            (s.decoders.contains(&decoder) && secs >= s.from_s).then_some((
                i,
                s.per_frame,
                s.frames,
            ))
        })?;
        let done = h.slowed.entry((rule, decoder)).or_default();
        (*done < frames).then(|| {
            *done += 1;
            (per_frame, h.generation)
        })
    })
    .flatten();
    if let Some((per_frame, generation)) = slow {
        wait(&key, generation, Some(Instant::now() + per_frame));
    }
}

/// Blocks until `until` (None: for ever), or until `path`'s hooks change.
fn wait(path: &str, generation: u64, until: Option<Instant>) {
    while until.is_none_or(|until| Instant::now() < until)
        && with(path, |h| h.generation) == Some(generation)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The trial smoke's hooks, from the environment (`trial` builds; the export worker calls it first):
/// `KLIPOLO_TRIAL_STALL_READS=<path>|<frame>|<ms or forever>|<readers>` and
/// `KLIPOLO_TRIAL_SLOW_SOFTWARE=<path>|<ms per frame>|<frames>|<decoders>` (from the file's start).
#[cfg(feature = "trial")]
pub fn from_env() {
    fn fields(name: &str) -> Option<Vec<String>> {
        let value = std::env::var(name).ok()?;
        let fields: Vec<String> = value.split('|').map(str::to_owned).collect();
        (fields.len() == 4).then_some(fields)
    }
    if let Some(f) = fields("KLIPOLO_TRIAL_STALL_READS") {
        let stall = match f[2].as_str() {
            "forever" => Stall::Forever,
            ms => Stall::For(Duration::from_millis(ms.parse().unwrap_or(0))),
        };
        let rule = ReadStall {
            readers: 0..f[3].parse().unwrap_or(1),
            at_frame: f[1].parse().unwrap_or(0),
            stall,
        };
        stall_reads(f[0].as_str(), [rule]);
    }
    if let Some(f) = fields("KLIPOLO_TRIAL_SLOW_SOFTWARE") {
        let rule = SlowSoftware {
            decoders: 0..f[3].parse().unwrap_or(1),
            from_s: 0.0,
            per_frame: Duration::from_millis(f[1].parse().unwrap_or(0)),
            frames: f[2].parse().unwrap_or(0),
        };
        slow_software(f[0].as_str(), [rule]);
    }
}
