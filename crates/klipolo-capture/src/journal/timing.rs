//! `timing.jsonl`: the audio runs' anchors, one line each, written once a second and read back by the manifest.

use std::collections::BTreeMap;
use std::io::{self, ErrorKind};
use std::path::Path;

use klipolo_model::time::Us;
use serde::{Deserialize, Serialize};

use super::for_each_line;
use crate::drift::Anchor;
use crate::media::SourceId;

/// One line of `timing.jsonl`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnchorRow<'a> {
    source: &'a str,
    run: u32,
    session_us: Us,
    frame: u64,
}

/// An anchor of run `run` of `source` as one `timing.jsonl` line:
/// `{"source":"mic","run":1,"sessionUs":1000000,"frame":48000}`.
pub fn anchor_line(source: &str, run: u32, a: &Anchor) -> Vec<u8> {
    let row = AnchorRow {
        source,
        run,
        session_us: a.session_us,
        frame: a.frame,
    };
    let mut out = serde_json::to_vec(&row).expect("an anchor serialises");
    out.push(b'\n');
    out
}

/// Every run's anchors, in the order they were written. Lines that don't parse (a torn last line) are skipped. A take
/// without `timing.jsonl` has no anchors: an empty map.
pub fn read_anchors(path: &Path) -> io::Result<BTreeMap<(SourceId, u32), Vec<Anchor>>> {
    let mut out: BTreeMap<(SourceId, u32), Vec<Anchor>> = BTreeMap::new();
    let mut skipped = 0_usize;
    let read = for_each_line(path, |_, line, _| {
        match serde_json::from_slice::<AnchorRow>(line) {
            Ok(row) => out
                .entry((row.source.to_owned(), row.run))
                .or_default()
                .push(Anchor {
                    session_us: row.session_us,
                    frame: row.frame,
                }),
            Err(_) => skipped += 1,
        }
    });
    match read {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        other => other?,
    }
    if skipped > 0 {
        tracing::warn!(skipped, "timing: skipped lines that don't parse");
    }
    Ok(out)
}
