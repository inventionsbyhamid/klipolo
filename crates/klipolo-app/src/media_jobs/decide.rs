//! What each of the open project's files needs (looking only: nothing is written until a copy starts), and whether a
//! finished copy is still its source's.

use super::*;

/// Identifies what copies depend on: the opening and its video files (a hash over their paths, no copy per edit).
pub(super) fn files_key(f: &ProjectFeed) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&f.project_id, f.opened).hash(&mut h);
    for asset in f
        .project
        .assets
        .iter()
        .filter(|a| a.kind == AssetKind::Video)
    {
        asset.path.hash(&mut h);
    }
    h.finish()
}

/// The project's video files (on disk, not inside packs), each once: the timeline's in its order (main clips, their
/// recordings' video and video layers, `media::wants`), then the rest.
pub(super) fn view_of(project: &Project) -> ProjectView {
    let mut seen = HashSet::new();
    let mut paths: Vec<Arc<str>> = Vec::new();
    let timeline = klipolo_engine::media::wants(project, 0)
        .into_iter()
        .map(|w| w.path);
    let rest = project
        .assets
        .iter()
        .filter(|a| a.kind == AssetKind::Video)
        .map(|a| a.path.clone());
    for path in timeline.chain(rest) {
        if matches!(AssetPath::parse(&path), Some(AssetPath::File(_))) && seen.insert(path.clone())
        {
            paths.push(path.into());
        }
    }
    ProjectView {
        paths: paths.into(),
    }
}

/// What `path` needs under `policy`, looking only: its finished copy, its unfinished job's progress, or a job to start.
/// No job folder is made here (it is made when the copy really starts).
/// - Nothing is probed where no copy can be made (`can_encode`: Windows and Linux today).
/// - Facts are probed once per version of the file (`FactKey`: path, size, time and number), whatever the GPU class, so
///   deciding again after the engine reports its class reads nothing again.
/// - A finished copy counts only when its record says it is of this version with these facts; otherwise it is the
///   cache's own stale data and is deleted, and the file gets a new job.
/// - `finished_only` (automatic copies are off): only a finished copy is looked for, and nothing is probed for a file
///   that has none; a file that would need a copy is `NoCopy`, so no row waits while copies are off.
pub(super) fn decide(
    root: &Path,
    path: &str,
    policy: &ProxyPolicy,
    probe: &dyn Fn(&Path) -> Result<SourceFacts, ProbeError>,
    cache: &FactsCache,
    finished_only: bool,
) -> Decision {
    if !policy.can_encode {
        return Decision::NoCopy;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return Decision::Unavailable;
    };
    let id = SourceId::of(&meta);
    let stem = stem_of(
        path,
        &meta,
        &CacheKind::Proxy {
            long_side: policy.long_side,
        },
    );
    let copy = proxy_path(root, &stem);
    if finished_only && !copy.is_file() {
        return Decision::NoCopy;
    }
    let key: FactKey = (Arc::from(path), id);
    let cached = lock(cache).get(&key).cloned();
    let facts = match cached {
        Some(facts) => facts,
        None => match probe(Path::new(path)) {
            Ok(facts) => {
                let facts = Arc::new(facts);
                let mut cache = lock(cache);
                if cache.len() >= FACTS_KEPT {
                    cache.clear();
                }
                cache.insert(key, facts.clone());
                facts
            }
            Err(ProbeError::Unreadable(_)) if !Path::new(path).is_file() => {
                return Decision::Unavailable;
            }
            Err(ProbeError::Unreadable(_)) if finished_only => return Decision::NoCopy,
            Err(ProbeError::Unreadable(message)) => return Decision::Unreadable(message),
            Err(ProbeError::TimedOut | ProbeError::Changing) => return Decision::Unavailable,
        },
    };
    // Facts of another version (it changed while it was probed): decided again at the next look.
    if facts.id.is_some_and(|probed| probed != id) {
        return Decision::Unavailable;
    }
    let Some(why) = needs_proxy(&facts, policy) else {
        return Decision::NoCopy;
    };
    let Some(rate) = proxy_rate(&facts) else {
        return Decision::NoCopy;
    };
    let fps = rate.f64();
    if copy.is_file() {
        match CopyRecord::read(&copy) {
            Some(record) if record.current(path, &copy, Some(id), Some(&facts)) => {
                return Decision::Done {
                    copy: copy.to_string_lossy().into(),
                    fps,
                    stem,
                    facts,
                    why,
                };
            }
            // Made from other facts, or without a record: never mapped, and gone (the cache's own data).
            _ => {
                tracing::info!(
                    file = %schedule::file_name(path),
                    "editing copies: a copy that doesn't match its source is removed"
                );
                remove_copy(&copy);
            }
        }
    }
    if finished_only {
        return Decision::NoCopy;
    }
    let progress = ProxyJob::open(&job_dir(root, &stem)).map_or(0.0, |job| {
        job.frames_done() as f64 / job.total_frames.max(1) as f64
    });
    Decision::Queue {
        stem,
        fps,
        progress,
        facts,
        why,
    }
}

/// Where a finished copy stands against its source now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopyState {
    /// Its record names this version of the file (and, given them, these facts): it may be mapped.
    Current,
    /// The source is there but is another version (or the copy has no record that reads): the copy is the cache's own
    /// stale data, to delete.
    Changed,
    /// The source can't be looked at now (an ejected drive, a share that dropped): the copy is unmapped and kept, and
    /// mapped again if the file comes back unchanged.
    Unreachable,
}

/// `copy` against `source` as the file is now (a stat; `facts`, of this version, when known). Called off the
/// schedule's lock: the source may be on a slow volume.
pub(super) fn copy_state(source: &str, copy: &Path, facts: Option<&SourceFacts>) -> CopyState {
    let Some(now) = source_id(source) else {
        return CopyState::Unreachable;
    };
    match CopyRecord::read(copy) {
        Some(record) if record.current(source, copy, Some(now), facts) => CopyState::Current,
        _ => CopyState::Changed,
    }
}

/// The facts of `path` as it is now: from the cache of probed versions, else probed (and kept). The stat comes first,
/// so a version is never mistaken for another.
pub(super) fn facts_now(
    path: &str,
    probe: &dyn Fn(&Path) -> Result<SourceFacts, ProbeError>,
    cache: &FactsCache,
) -> Result<Arc<SourceFacts>, ProbeError> {
    let id = source_id(path).ok_or_else(|| ProbeError::Unreadable("not there".into()))?;
    let key: FactKey = (Arc::from(path), id);
    if let Some(facts) = lock(cache).get(&key).cloned() {
        return Ok(facts);
    }
    let facts = Arc::new(probe(Path::new(path))?);
    let mut cache = lock(cache);
    if cache.len() >= FACTS_KEPT {
        cache.clear();
    }
    cache.insert(key, facts.clone());
    Ok(facts)
}

/// Marks a copy as just used (the tidy removes the least recently used first). The copy's keyframe index names it by
/// its size and number, so this doesn't lose it.
pub(super) fn touch(path: &Path) {
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}
