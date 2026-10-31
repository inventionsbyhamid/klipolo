//! The open project as chat reads and edits it (`klipolo_app::chat::Projects`), through the path every edit takes: one
//! undo step, the engine's feed, the save. Chat names where the project stood (`At`: the project, its opening, its
//! revision) and an edit made on a revision the project left is refused, never rebased.

use klipolo_app::chat::{At, CommitError, Committed, ProjectNow, Projects};
use klipolo_chat::{LibraryItem, MediaKind};

use crate::pack::PackCategory;

use super::apply::commit_reviewed;
use super::*;

/// Where `open` stands in `state`.
pub(super) fn at_of(state: &ProjectSessionState, open: &OpenProject) -> At {
    At {
        project_id: open.document.project().id.clone(),
        opening: state.opens.load(Ordering::SeqCst),
        revision: open.document.revision(),
    }
}

impl ProjectSessionState {
    /// `f` on the open project when it still stands `at`, then where it stands.
    fn chat_at(
        &self,
        at: &At,
        f: impl FnOnce(&mut OpenProject) -> Result<SnapshotPatch, Said>,
    ) -> Result<Committed, CommitError> {
        let done = with_open(self, |open| {
            if at_of(self, open) != *at {
                return Ok(Err(CommitError::Moved));
            }
            Ok(f(open)
                .map_err(CommitError::Refused)
                .map(|patch| Committed {
                    patch,
                    at: at_of(self, open),
                    project: open.document.shared(),
                }))
        });
        done.unwrap_or(Err(CommitError::NoProject))
    }
}

impl Projects for ProjectSessionState {
    fn now(&self) -> Option<ProjectNow> {
        let open = lock(self);
        let open = open.as_ref()?;
        Some(ProjectNow {
            at: at_of(self, open),
            project: open.document.shared(),
            file: open.file.clone(),
            read_only: open.read_only,
        })
    }

    fn commit(
        &self,
        at: &At,
        tx: &Transaction,
        next: Option<Project>,
    ) -> Result<Committed, CommitError> {
        self.chat_at(at, |open| commit_reviewed(open, tx, next))
    }

    fn undo(&self, at: &At) -> Result<Committed, CommitError> {
        self.chat_at(at, |open| step(open, false))
    }

    /// The pack this install has, as the library lists it, each item as the asset a project adds.
    fn pack(&self) -> Vec<LibraryItem> {
        let pack = ProjectSessionState::pack(self);
        (pack.items().into_iter())
            .filter_map(|item| {
                let asset = pack.asset(&item.id)?;
                let kind = match item.category {
                    PackCategory::Music => MediaKind::Music,
                    PackCategory::Sound => MediaKind::Sound,
                    PackCategory::Sticker => MediaKind::Image,
                };
                Some(LibraryItem {
                    name: item.name,
                    kind,
                    asset,
                    pack: true,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use klipolo_model::Op;
    use klipolo_model::ops::Origin;
    use klipolo_model::project::{Asset, AssetKind};
    use klipolo_model::time::RangeUs;
    use klipolo_model::timeline::{Clip, ClipSource};

    use super::*;
    use crate::project::tests::rename;

    /// "talk.mov", 30 s, played whole.
    fn talk() -> Project {
        let mut p = Project::new("Talk");
        p.assets = vec![Asset {
            id: "talk".into(),
            kind: AssetKind::Video,
            path: "/media/talk.mov".into(),
            name: "talk.mov".into(),
            duration_us: 30_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
        }];
        let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, 30_000_000);
        clip.id = "t".into();
        p.timeline.main = vec![clip];
        p
    }

    /// Chat's cut of 10 to 20 s.
    fn cut() -> Transaction {
        Transaction {
            label: "chat.cutRange".into(),
            ops: vec![Op::DeleteRanges {
                ranges: vec![RangeUs {
                    start: 10_000_000,
                    end: 20_000_000,
                }],
            }],
            origin: Some(Origin::Chat),
            ..Default::default()
        }
    }

    fn engine_revision(state: &ProjectSessionState) -> u64 {
        state.subscribe().borrow().as_ref().unwrap().revision
    }

    #[test]
    fn apply_is_one_undo_step() {
        let state = opened(talk());
        let now = state.now().unwrap();
        // The proposal's review, as chat keeps it: Apply records that project as it is.
        let review = cut().review(&now.project).unwrap();
        let done = state
            .commit(&now.at, &cut(), Some(review.after.clone()))
            .unwrap();
        assert_eq!(done.patch.base_revision, 0);
        assert_eq!(done.at.revision, 1);
        assert_eq!(*done.project, review.after);
        assert_eq!(engine_revision(&state), 1, "the engine has it");
        let open = lock(&state);
        let document = &open.as_ref().unwrap().document;
        assert_eq!(document.undo_steps(), 1, "one undo step");
        assert_eq!(
            document.history().undo_label.as_deref(),
            Some("chat.cutRange")
        );
        drop(open);
        // Undo right after Apply restores the cut footage.
        let back = state.undo(&done.at).unwrap();
        assert_eq!(*back.project, *now.project);
    }

    #[test]
    fn a_revision_the_project_left_is_refused() {
        let state = opened(talk());
        let now = state.now().unwrap();
        with_open(&state, |open| {
            super::super::apply::apply_and_commit(open, &rename("Mine"))
        })
        .unwrap();
        assert_eq!(
            state.commit(&now.at, &cut(), None).unwrap_err(),
            CommitError::Moved
        );
        assert_eq!(state.undo(&now.at).unwrap_err(), CommitError::Moved);
        let after = state.now().unwrap();
        assert_eq!(after.project.name, "Mine", "the hand edit stays");
        assert_eq!(after.project.timeline.main.len(), 1, "nothing was cut");
        // Another opening of the project: the same revision number is another project.
        set_open(&state, open_file_for(talk())).unwrap();
        let reopened = state.now().unwrap();
        assert_eq!(reopened.at.revision, 0);
        let stale = At {
            revision: 0,
            ..now.at
        };
        assert_eq!(
            state.commit(&stale, &cut(), None).unwrap_err(),
            CommitError::Moved
        );
        assert!(state.commit(&reopened.at, &cut(), None).is_ok());
    }

    #[test]
    fn a_read_only_project_refuses_chat() {
        let state = opened(talk());
        lock(&state).as_mut().unwrap().read_only = true;
        let now = state.now().unwrap();
        assert!(now.read_only);
        assert!(matches!(
            state.commit(&now.at, &cut(), None),
            Err(CommitError::Refused(_))
        ));
    }

    #[test]
    fn no_project_is_said_so() {
        let state = opened(talk());
        let now = state.now().unwrap();
        set_open(&state, None).unwrap();
        assert!(state.now().is_none());
        assert_eq!(
            state.commit(&now.at, &cut(), None).unwrap_err(),
            CommitError::NoProject
        );
    }

    fn open_file_for(project: Project) -> Option<OpenProject> {
        Some(OpenProject {
            file: std::env::temp_dir().join(format!("{}.{PROJECT_EXTENSION}", project.id)),
            document: Document::new(project).unwrap(),
            read_only: false,
        })
    }

    #[test]
    fn chat_names_the_pack_this_install_has() {
        use klipolo_chat::MediaKind;
        let state = opened(talk());
        assert!(Projects::pack(&state).is_empty(), "no pack read yet");
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/library");
        state.set_pack(crate::pack::Pack::read(&dir));
        let pack = Projects::pack(&state);
        let count = |kind| pack.iter().filter(|i| i.kind == kind).count();
        assert_eq!(
            (
                count(MediaKind::Music),
                count(MediaKind::Sound),
                count(MediaKind::Image)
            ),
            (2, 8, 6)
        );
        for item in &pack {
            assert!(item.pack);
            assert!(
                item.asset.id.starts_with("klipolo-pack-"),
                "{}",
                item.asset.id
            );
            assert!(Path::new(&item.asset.path).is_file(), "{}", item.asset.path);
        }
    }
}
