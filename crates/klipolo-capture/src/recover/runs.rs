//! The run files a repair works on: the ones the journal's `RunStarted` lines name, or, without a journal, the ones in
//! each source's folder, placed end to end (approximate).

use std::collections::BTreeMap;
use std::io::{self, ErrorKind};

use klipolo_model::time::Us;

use crate::journal::{Entry, JournalRead};
use crate::media::{AudioFormat, SourceId, StreamInfo};
use crate::session::{self, FileKey, SessionConfig, SessionDir};

/// A run file to repair, as the journal names it or as found in the folder.
pub(crate) struct RunFile {
    pub(crate) source: SourceId,
    pub(crate) run: u32,
    pub(crate) file: FileKey,
    pub(crate) video: bool,
    pub(crate) stream: Option<StreamInfo>,
    pub(crate) format: Option<AudioFormat>,
    /// Its first sample's session time, as the journal says (None without one).
    pub(crate) offset_us: Option<Us>,
}

/// The runs the journal names, once each (a later line for the same file wins), in the order they started.
pub(crate) fn told(journal: &JournalRead) -> Vec<RunFile> {
    let mut runs: Vec<RunFile> = Vec::new();
    for entry in &journal.entries {
        if let Entry::RunStarted {
            source,
            run,
            file,
            offset_us,
            video,
            audio,
        } = entry
        {
            runs.retain(|r| &r.file != file);
            runs.push(RunFile {
                source: source.clone(),
                run: *run,
                file: file.clone(),
                video: file.ends_with(".mp4"),
                stream: video.clone(),
                format: *audio,
                offset_us: Some(*offset_us),
            });
        }
    }
    runs
}

/// Without a journal: every `run-NN.mp4` or `run-NN.caf` in each configured source's folder, in run order.
pub(super) fn found(dir: &SessionDir, cfg: &SessionConfig) -> io::Result<Vec<RunFile>> {
    let mut runs = Vec::new();
    for source in cfg.sources.iter().filter(|s| session::is_plain_name(&s.id)) {
        let entries = match std::fs::read_dir(dir.root.join(&source.id)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        let mut mine = Vec::new();
        for entry in entries {
            let name = entry?.file_name();
            let Some((run, video)) = name.to_str().and_then(run_name) else {
                continue;
            };
            mine.push(RunFile {
                source: source.id.clone(),
                run,
                file: format!("{}/{}", source.id, name.to_string_lossy()),
                video,
                stream: None,
                format: None,
                offset_us: None,
            });
        }
        mine.sort_by_key(|r| r.run);
        runs.extend(mine);
    }
    Ok(runs)
}

/// "run-02.caf" → (2, sound); "run-01.mp4" → (1, video).
fn run_name(name: &str) -> Option<(u32, bool)> {
    let (run, ext) = name.strip_prefix("run-")?.split_once('.')?;
    let video = match ext {
        "mp4" => true,
        "caf" => false,
        _ => return None,
    };
    Some((run.parse().ok()?, video))
}

/// Without a journal, each source's runs placed end to end from 0 (approximate), as the manifest reads runs.
pub(super) fn placed(runs: &[RunFile], durations: &BTreeMap<FileKey, Us>) -> Vec<Entry> {
    let mut ends: BTreeMap<&str, Us> = BTreeMap::new();
    let mut out = Vec::new();
    for r in runs {
        let Some(&duration) = durations.get(&r.file) else {
            continue;
        };
        let end = ends.entry(&r.source).or_insert(0);
        out.push(Entry::RunStarted {
            source: r.source.clone(),
            run: r.run,
            file: r.file.clone(),
            offset_us: *end,
            video: None,
            audio: None,
        });
        *end += duration;
    }
    out
}

/// Where `run` starts in the recording: the journal's offset, or else after its source's earlier runs that were
/// measured (as `placed` puts them).
pub(super) fn start_us(runs: &[RunFile], durations: &BTreeMap<FileKey, Us>, run: &RunFile) -> Us {
    run.offset_us.unwrap_or_else(|| {
        runs.iter()
            .filter(|r| r.source == run.source && r.run < run.run)
            .filter_map(|r| durations.get(&r.file))
            .sum()
    })
}
