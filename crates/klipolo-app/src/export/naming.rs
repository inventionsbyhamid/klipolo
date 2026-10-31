//! Export file names: `<project name> 16x9.mp4`, `9x16`, `1x1` (`.mov` for Master), never overwriting a file: a name
//! taken on the disk, or by another file of the same export, gets the next free one (`<name> 16x9 2.mp4`). The engine
//! renames each finished file into place the same way, so a name taken while it runs is never written over either.

use std::collections::HashSet;
use std::path::Path;

use klipolo_engine::api::{ExportQuality, PlannedFile};
use klipolo_engine::export::naming::{free_path, sanitize};
use klipolo_engine::export::quality::{EncoderCaps, encode_settings, output_size};
use klipolo_model::project::{FormatKind, Project};

/// The word a file's name gives its format. No colon: a Mac's file names can't hold one.
pub fn format_word(format: FormatKind) -> &'static str {
    match format {
        FormatKind::Landscape => "16x9",
        FormatKind::Portrait => "9x16",
        FormatKind::Square => "1x1",
    }
}

/// One file per format in `dir`, in 16:9, 9:16, 1:1 order (each once), at its size; nothing is written.
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
            let path = free_path(dir, &format!("{name} {}", format_word(format)), ext, &taken);
            taken.insert(path.clone());
            PlannedFile {
                format,
                width,
                height,
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: path.display().to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(files: &[PlannedFile]) -> Vec<&str> {
        files.iter().map(|f| f.name.as_str()).collect()
    }

    #[test]
    fn each_format_gets_its_own_file_named_by_its_shape() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::new("Rosa studio visit");
        let all = [
            FormatKind::Square,
            FormatKind::Portrait,
            FormatKind::Landscape,
            FormatKind::Portrait,
        ];
        let files = plan_files(&project, &all, ExportQuality::High, dir.path());
        assert_eq!(
            names(&files),
            [
                "Rosa studio visit 16x9.mp4",
                "Rosa studio visit 9x16.mp4",
                "Rosa studio visit 1x1.mp4"
            ],
            "16:9, 9:16, 1:1, each once"
        );
        assert_eq!(
            files
                .iter()
                .map(|f| (f.width, f.height))
                .collect::<Vec<_>>(),
            [(1920, 1080), (1080, 1920), (1080, 1080)]
        );
        assert_eq!(
            files[1].path,
            dir.path()
                .join("Rosa studio visit 9x16.mp4")
                .display()
                .to_string()
        );
        let master = plan_files(
            &project,
            &[FormatKind::Portrait],
            ExportQuality::Master,
            dir.path(),
        );
        assert_eq!(
            names(&master),
            ["Rosa studio visit 9x16.mov"],
            "ProRes goes in a .mov"
        );
        assert_eq!(
            (
                format_word(FormatKind::Landscape),
                format_word(FormatKind::Portrait),
                format_word(FormatKind::Square)
            ),
            ("16x9", "9x16", "1x1")
        );
    }

    #[test]
    fn an_existing_file_is_never_planned_over() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Demo 9x16.mp4"), b"an earlier export").unwrap();
        std::fs::write(dir.path().join("Demo 9x16 2.mp4"), b"and another").unwrap();
        let files = plan_files(
            &Project::new("Demo"),
            &[FormatKind::Portrait],
            ExportQuality::High,
            dir.path(),
        );
        assert_eq!(names(&files), ["Demo 9x16 3.mp4"]);
        assert_eq!(
            std::fs::read(dir.path().join("Demo 9x16.mp4")).unwrap(),
            b"an earlier export",
            "planning writes nothing"
        );
    }

    #[test]
    fn a_name_that_cant_be_a_file_name_is_made_safe() {
        let dir = tempfile::tempdir().unwrap();
        let files = plan_files(
            &Project::new("../Q3: a/b"),
            &[FormatKind::Square],
            ExportQuality::High,
            dir.path(),
        );
        assert_eq!(
            files[0].name, "-Q3- a-b 1x1.mp4",
            "no slash, colon or leading dot"
        );
        assert_eq!(
            Path::new(&files[0].path).parent(),
            Some(dir.path()),
            "it stays in the folder"
        );
    }
}
