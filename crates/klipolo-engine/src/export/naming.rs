//! Export file names: `<project name> (Landscape|Vertical|Square).mp4` (`.mov` for Master) in the videos folder's
//! Klipolo/Exports (~/Movies on a Mac), never overwriting.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use klipolo_model::project::{FormatKind, Project};

use super::quality::{EncoderCaps, encode_settings, output_size};
use crate::api::{ExportQuality, PlannedFile};

pub fn format_word(format: FormatKind) -> &'static str {
    match format {
        FormatKind::Landscape => "Landscape",
        FormatKind::Portrait => "Vertical",
        FormatKind::Square => "Square",
    }
}

/// A project name that is safe as a file name on every OS (`klipolo_platform::paths::safe_file_name`): no slashes,
/// colons, `< > " \ | ? *` or control characters, no Windows device name; no leading or trailing dots or spaces; at
/// most 100 characters; an empty result becomes "Untitled video".
pub use klipolo_platform::paths::safe_file_name as sanitize;

/// `dir/<base>.<ext>`, else `dir/<base> 2.<ext>`, `… 3`…: the first name neither on disk nor in `taken`.
pub fn free_path(dir: &Path, base: &str, ext: &str, taken: &HashSet<PathBuf>) -> PathBuf {
    (1..)
        .map(|n| {
            dir.join(if n == 1 {
                format!("{base}.{ext}")
            } else {
                format!("{base} {n}.{ext}")
            })
        })
        .find(|p| !p.exists() && !taken.contains(p))
        .expect("an unused name")
}

/// One file per format, in landscape, portrait, square order, at its quality's size; Master files are `.mov`.
pub fn plan_files(
    project: &Project,
    formats: &[FormatKind],
    quality: ExportQuality,
    dir: &Path,
) -> Vec<PlannedFile> {
    let name = sanitize(&project.name);
    let mut sorted = formats.to_vec();
    sorted.sort();
    sorted.dedup();
    let mut taken = HashSet::new();
    sorted
        .into_iter()
        .map(|format| {
            let (width, height) = output_size(project, format);
            let ext =
                encode_settings(quality, (width, height), 30, &EncoderCaps::ASSUMED).extension();
            let path = free_path(
                dir,
                &format!("{name} ({})", format_word(format)),
                ext,
                &taken,
            );
            taken.insert(path.clone());
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            PlannedFile {
                format,
                width,
                height,
                name,
                path: path.display().to_string(),
            }
        })
        .collect()
}
