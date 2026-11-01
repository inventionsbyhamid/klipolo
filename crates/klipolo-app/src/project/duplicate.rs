//! Duplicate: a copy of a project, saved in the library beside the others and opened, to make another video from the
//! same footage (several shorts from one long video). The copy is the project as it stands, its latest edit in: the
//! same media, captions, edits and word fixes under a new id, named "<name> copy" ("<name> copy 2" and on while that is
//! taken). Analysis is kept per file, so the copy has it too. Its undo history starts fresh.

use std::path::Path;

use klipolo_model::coded::Said;
use klipolo_model::history::Snapshot;
use klipolo_model::ids::new_id;
use klipolo_model::project::Project;
use klipolo_platform::paths::{FILE_NAME_MAX, safe_file_name};

use super::{
    PROJECT_EXTENSION, ProjectSessionState, create_in, is_read_only, lock, read, read_only_refusal,
};

/// The name a copy of `name` takes in `library`: "<name> copy", else "<name> copy 2" and on, the first whose file isn't
/// there yet. A copy of a copy counts on from what it copied ("Trip copy" makes "Trip copy 2", never "Trip copy copy").
/// A long name is shortened, never the word, to the 100 characters a file's name keeps (`FILE_NAME_MAX`): each name
/// tried then has a file of its own, so the search ends.
pub(crate) fn copy_name(library: &Path, name: &str) -> String {
    let base = copy_base(name.trim());
    (1..)
        .map(|n| {
            let ending = match n {
                1 => " copy".to_string(),
                n => format!(" copy {n}"),
            };
            let kept: String = base
                .chars()
                .take(FILE_NAME_MAX.saturating_sub(ending.chars().count()))
                .collect();
            format!("{}{ending}", kept.trim_end())
        })
        .find(|name| {
            !library
                .join(format!("{}.{PROJECT_EXTENSION}", safe_file_name(name)))
                .exists()
        })
        .expect("a free name")
}

/// `name` without a copy's ending (" copy", " copy 7"); as it is otherwise ("Take 2" stays "Take 2").
fn copy_base(name: &str) -> &str {
    let unnumbered = match name.rsplit_once(' ') {
        Some((head, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => head,
        _ => name,
    };
    unnumbered
        .strip_suffix(" copy")
        .filter(|base| !base.trim().is_empty())
        .unwrap_or(name)
}

/// `project` as a copy named `name`: a new id; everything else as it is.
pub(crate) fn copy_of(project: &Project, name: String) -> Project {
    let mut copy = project.clone();
    copy.id = new_id();
    copy.name = name;
    copy
}

/// Duplicates the project in `file` (None, or the open project's own file: the open project as it stands, its latest
/// edit in) into `library` and opens the copy, the open one saved and closed first as for any open: its snapshot. A
/// project from a newer Klipolo isn't copied (`model.readOnly`): this build would leave out what it doesn't know.
pub(crate) fn duplicate_in(
    state: &ProjectSessionState,
    library: &Path,
    file: Option<&Path>,
) -> Result<Snapshot, Said> {
    let open = {
        let guard = lock(state);
        guard
            .as_ref()
            .filter(|open| file.is_none_or(|file| open.file == file))
            .map(|open| (open.document.project().clone(), open.read_only))
    };
    let source = match (open, file) {
        (Some((_, true)), _) => return Err(read_only_refusal()),
        (Some((project, false)), _) => project,
        (None, Some(file)) => {
            let mut project = read(file)?;
            state.pack().repoint(&mut project);
            project
        }
        (None, None) => return Err(Said::code("project.none")),
    };
    if is_read_only(&source) {
        return Err(read_only_refusal());
    }
    let name = copy_name(library, &source.name);
    create_in(state, library, copy_of(&source, name))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use klipolo_model::Op;
    use klipolo_model::project::{Asset, AssetKind};
    use klipolo_model::{Transaction, history::Document};
    use klipolo_platform::durable::Durability;

    use super::super::{OpenProject, apply_and_commit, open_file, save, set_open, with_open};
    use super::*;
    use crate::pack::Pack;

    fn asset() -> Asset {
        Asset {
            id: "v".into(),
            kind: AssetKind::Video,
            path: "/media/talk.mp4".into(),
            name: "talk.mp4".into(),
            duration_us: 60_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
        }
    }

    /// "Talk" open from `library`, with its footage imported (an edit not yet saved).
    fn talk_open(library: &Path) -> ProjectSessionState {
        let state = ProjectSessionState::default();
        let file = library.join("Talk.klipolo");
        save(&file, &Project::new("Talk"), Durability::Full).unwrap();
        set_open(
            &state,
            Some(OpenProject {
                document: Document::new(Project::new("Talk")).unwrap(),
                file,
                read_only: false,
            }),
        )
        .unwrap();
        let import = Transaction {
            label: "media.import".into(),
            ops: vec![Op::AddAssets {
                assets: vec![asset()],
            }],
            ..Default::default()
        };
        with_open(&state, |open| apply_and_commit(open, &import)).unwrap();
        state
    }

    fn open_now(state: &ProjectSessionState) -> (Project, std::path::PathBuf) {
        let guard = lock(state);
        let open = guard.as_ref().unwrap();
        (open.document.project().clone(), open.file.clone())
    }

    #[test]
    fn the_open_project_is_copied_as_it_stands_and_the_copy_opens() {
        let dir = tempfile::tempdir().unwrap();
        let state = talk_open(dir.path());
        let (talk, talk_file) = open_now(&state);
        let snapshot = duplicate_in(&state, dir.path(), None).unwrap();
        assert_eq!(snapshot.project.name, "Talk copy");
        assert_eq!(snapshot.opening, Some(2), "the copy opened");
        assert_ne!(snapshot.project.id, talk.id, "a project of its own");
        assert_eq!(
            snapshot.project.assets, talk.assets,
            "the same media, the edit not yet saved in"
        );
        let (_, file) = open_now(&state);
        assert_eq!(file, dir.path().join("Talk copy.klipolo"));
        assert_eq!(read(&file).unwrap().assets, talk.assets, "saved");
        // The one copied was saved as it closed, and is still itself.
        let saved = read(&talk_file).unwrap();
        assert_eq!(
            (saved.name.as_str(), saved.id.as_str()),
            ("Talk", talk.id.as_str())
        );
        assert_eq!(saved.assets, talk.assets);
    }

    #[test]
    fn copies_count_on_from_the_project_copied() {
        let dir = tempfile::tempdir().unwrap();
        let state = talk_open(dir.path());
        let talk = dir.path().join("Talk.klipolo");
        let names: Vec<String> = (0..3)
            .map(|_| {
                duplicate_in(&state, dir.path(), Some(&talk))
                    .unwrap()
                    .project
                    .name
            })
            .collect();
        assert_eq!(names, ["Talk copy", "Talk copy 2", "Talk copy 3"]);
        // A copy of a copy is the next copy, not "Talk copy copy".
        let again = duplicate_in(&state, dir.path(), None).unwrap();
        assert_eq!(again.project.name, "Talk copy 4");
        assert!(dir.path().join("Talk copy 4.klipolo").is_file());
    }

    #[test]
    fn a_card_copies_the_project_in_its_file_with_none_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut trip = Project::new("Trip");
        trip.assets.push(asset());
        let file = dir.path().join("Trip.klipolo");
        save(&file, &trip, Durability::Full).unwrap();
        let state = ProjectSessionState::default();
        let snapshot = duplicate_in(&state, dir.path(), Some(&file)).unwrap();
        assert_eq!(snapshot.project.name, "Trip copy");
        assert_eq!(snapshot.project.assets, trip.assets);
        assert_ne!(snapshot.project.id, trip.id);
        assert_eq!(
            read(&file).unwrap(),
            trip,
            "the card's own file is untouched"
        );
        let missing = duplicate_in(
            &ProjectSessionState::default(),
            dir.path(),
            Some(&dir.path().join("Gone.klipolo")),
        );
        assert!(
            missing.is_err(),
            "a file that isn't there is said, not copied"
        );
        assert_eq!(
            duplicate_in(&ProjectSessionState::default(), dir.path(), None).unwrap_err(),
            Said::code("project.none")
        );
    }

    #[test]
    fn a_project_from_a_newer_klipolo_is_not_copied() {
        let dir = tempfile::tempdir().unwrap();
        let mut newer = Project::new("Newer");
        newer.requires.insert("templates@1".into());
        let file = dir.path().join("Newer.klipolo");
        save(&file, &newer, Durability::Full).unwrap();
        let state = ProjectSessionState::default();
        let refused = Said::code("model.readOnly");
        assert_eq!(
            duplicate_in(&state, dir.path(), Some(&file)).unwrap_err(),
            refused
        );
        set_open(&state, Some(open_file(&file, &Pack::default()).unwrap())).unwrap();
        assert_eq!(duplicate_in(&state, dir.path(), None).unwrap_err(), refused);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no copy was written"
        );
    }

    #[test]
    fn a_copys_name_stays_a_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(copy_name(dir.path(), "Take 2"), "Take 2 copy");
        assert_eq!(copy_name(dir.path(), " copy"), "copy copy");
        assert_eq!(copy_name(dir.path(), "Trip copy 12"), "Trip copy");
        let long = "a".repeat(200);
        let copy = copy_name(dir.path(), &long);
        assert_eq!(copy.chars().count(), 100, "as long as a file name can be");
        assert!(copy.ends_with("a copy"));
        // A name made safe for its file still finds the free one.
        std::fs::write(
            dir.path()
                .join(format!("{}.klipolo", safe_file_name("a/b copy"))),
            "",
        )
        .unwrap();
        assert_eq!(copy_name(dir.path(), "a/b"), "a/b copy 2");
    }

    /// A file's name keeps 100 characters of a project's: a copy's ending stays in it, so a long name is copied, and
    /// copied again, each into a file of its own (the search for a free name never runs out).
    #[test]
    fn a_long_name_is_copied_again_each_into_its_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(ProjectSessionState::default());
        create_in(&state, dir.path(), Project::new("a".repeat(200))).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (copier, library) = (state.clone(), dir.path().to_path_buf());
        std::thread::spawn(move || {
            for _ in 0..2 {
                let copy = duplicate_in(&copier, &library, None).map(|s| s.project.name);
                let _ = tx.send(copy);
            }
        });
        let names: Vec<String> = (0..2)
            .map(|_| {
                rx.recv_timeout(Duration::from_secs(20))
                    .expect("a copy within 20 s")
                    .unwrap()
            })
            .collect();
        assert_eq!(
            names,
            [
                format!("{} copy", "a".repeat(95)),
                format!("{} copy 2", "a".repeat(93))
            ]
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
    }
}
