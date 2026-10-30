//! The worker's project frames in the tests: the snapshot, the contributions and the stills written into its `in/` as
//! the app writes them.

use klipolo_engine::make::frames::ContribEntry;
use klipolo_engine::make::protocol::{FrameSource, InputName};
use klipolo_engine::make::session::IN;
use klipolo_model::project::{AssetKind, Project};

use super::{FrameView, InProcessWorker};

impl InProcessWorker {
    /// `project` drawn from the stills the app decoded: `still` for every video asset at every time (its clips start at
    /// source 0, so a source time is the timeline time).
    pub async fn frames_project_with_stills(
        &mut self,
        project: Project,
        times: &[f64],
        still: image::RgbaImage,
    ) -> Vec<FrameView> {
        let videos: Vec<String> = project
            .assets
            .iter()
            .filter(|a| a.kind == AssetKind::Video)
            .map(|a| a.id.clone())
            .collect();
        let mut index = Vec::new();
        let mut files = Vec::new();
        for (i, (asset, t)) in videos
            .iter()
            .flat_map(|a| times.iter().map(move |t| (a, t)))
            .enumerate()
        {
            let file = format!("v{i}.png");
            files.push((file.clone(), still.clone()));
            index.push(serde_json::json!({"asset": asset, "sourceAt": t, "file": file}));
        }
        self.write_project(&project, &[], &files, serde_json::json!({"videos": index}));
        self.frames(project_source(), times, 512).await
    }

    /// `project` with the contribution `contrib`, and no video.
    pub async fn frames_project_with_contrib(
        &mut self,
        project: Project,
        contrib: ContribEntry,
        times: &[f64],
    ) -> Vec<FrameView> {
        self.write_project(&project, &[contrib], &[], serde_json::json!({}));
        self.frames(project_source(), times, 512).await
    }

    /// `project` with no contribution and no video.
    pub async fn frames_project(
        &mut self,
        project: Project,
        times: &[f64],
        long_edge: u32,
    ) -> Vec<FrameView> {
        self.write_project(&project, &[], &[], serde_json::json!({}));
        self.frames(project_source(), times, long_edge).await
    }

    pub(super) fn write_project(
        &self,
        p: &Project,
        contribs: &[ContribEntry],
        stills: &[(String, image::RgbaImage)],
        index: serde_json::Value,
    ) {
        let dir = self.session.join(IN);
        std::fs::write(dir.join("snapshot.json"), serde_json::to_vec(p).unwrap()).unwrap();
        std::fs::write(
            dir.join("contribs.json"),
            serde_json::to_vec(contribs).unwrap(),
        )
        .unwrap();
        let folder = dir.join("stills");
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir(&folder).unwrap();
        for (name, image) in stills {
            image.save(folder.join(name)).unwrap();
        }
        std::fs::write(
            folder.join("stills.json"),
            serde_json::to_vec(&index).unwrap(),
        )
        .unwrap();
    }
}

pub(super) fn project_source() -> FrameSource {
    let name = |n: &str| InputName::new(n).unwrap();
    FrameSource::Project {
        snapshot: name("snapshot.json"),
        contribs: name("contribs.json"),
        stills: name("stills"),
    }
}
