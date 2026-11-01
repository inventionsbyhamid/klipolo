//! The library on disk: `~/Movies/Klipolo`, one `<name>.klipolo` file per project, and reading a project file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use klipolo_model::coded::Said;
use klipolo_model::load_project;
use klipolo_model::project::Project;
use klipolo_model::summary::problem;
use klipolo_platform::paths::{safe_file_name, videos_dir};

use super::PROJECT_EXTENSION;

/// `Klipolo` in the videos folder (`~/Movies/Klipolo` on a Mac), made when missing.
pub(crate) fn library_dir() -> Result<PathBuf, Said> {
    let dir = videos_dir()
        .ok_or_else(|| Said::code("project.noVideosFolder"))?
        .join("Klipolo");
    fs::create_dir_all(&dir).map_err(|error| {
        tracing::warn!(%error, "the projects folder couldn't be made");
        Said::code("project.libraryFailed")
    })?;
    Ok(dir)
}

/// A file for a new project named `name` in `library`, and the name the project takes with it: `<name>.klipolo` and
/// `name`, or `<name> 2.klipolo` and `<name> 2` and so on when that is taken, so the project and its file say the same
/// number. The file's name is made safe as a file name, so it can't reach outside the library.
pub(crate) fn new_file(library: &Path, name: &str) -> (PathBuf, String) {
    let stem = safe_file_name(name);
    (1..)
        .map(|n| match n {
            1 => (
                library.join(format!("{stem}.{PROJECT_EXTENSION}")),
                name.to_owned(),
            ),
            n => (
                library.join(format!("{stem} {n}.{PROJECT_EXTENSION}")),
                format!("{name} {n}"),
            ),
        })
        .find(|(file, _)| !file.exists())
        .expect("a free name")
}

/// The project in `file`: `project.cantOpen` when the file can't be read (the reason goes to the log), else the model's
/// code when it doesn't load (`model.unreadable` for a file that isn't a project, or not even text).
pub(crate) fn read(file: &Path) -> Result<Project, Said> {
    let text = fs::read_to_string(file).map_err(|error| {
        if error.kind() == io::ErrorKind::InvalidData {
            return Said::code("model.unreadable");
        }
        tracing::warn!(%error, "a project file couldn't be read");
        Said::code(problem::CANT_OPEN)
    })?;
    load_project(&text).map_err(|e| Said::Coded(e.coded()))
}
