//! Scenes in the worker's tests: `CheckScene` from files written into its `in/` as the app writes them
//! (the scene's JSON, each SVG as its name, a line feed and its text, and `contribs.json`), what it answered, and
//! frames of a project placing the draft it checked.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use klipolo_engine::make::budget::Cost;
use klipolo_engine::make::frames::ContribEntry;
use klipolo_engine::make::protocol::{
    DraftSpec, FrameSource, FramesSpec, InputName, Key, MeasureSpec, WorkerReply, WorkerRequest,
};
use klipolo_engine::make::session::IN;
use klipolo_engine::registry::guard::Diagnostic;
use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::project::{FormatKind, Project};

use super::{FrameView, InProcessWorker};

/// The definition a scene draft is checked under (the app's draft id).
pub fn scene_draft_def() -> ContribDef {
    ContribDef::new("draft.local/scene.draft", ContribKind::Scene, "Draft")
}

/// What `CheckScene` answered.
pub struct SceneReply {
    pub lines: Vec<String>,
    pub ok: bool,
    pub cost: Option<Cost>,
    pub diagnostics: Vec<Diagnostic>,
    pub key: Option<Key>,
    /// The normalised bytes it stored.
    pub scene: Option<Vec<u8>>,
}

impl SceneReply {
    fn of(lines: Vec<String>) -> Self {
        let mut reply = Self {
            lines,
            ok: false,
            cost: None,
            diagnostics: Vec::new(),
            key: None,
            scene: None,
        };
        for line in &reply.lines {
            match serde_json::from_str(line) {
                Ok(WorkerReply::Compiled {
                    ok,
                    cost,
                    diagnostics,
                    key,
                    scene,
                    ..
                }) => {
                    reply.ok = ok;
                    reply.cost = cost;
                    reply.diagnostics = diagnostics;
                    reply.key = key;
                    reply.scene = scene.map(|s| s.into_bytes());
                }
                Ok(WorkerReply::Failed { reason, .. }) => panic!("the check failed: {reason}"),
                _ => {}
            }
        }
        reply
    }

    /// Its thumbnail, decoded.
    pub fn thumbnail(&self) -> Option<image::RgbImage> {
        self.lines.iter().find_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            let b64 = v["jpeg"].as_str().filter(|_| v["op"] == "thumbnail")?;
            let bytes = BASE64.decode(b64).ok()?;
            Some(image::load_from_memory(&bytes).ok()?.to_rgb8())
        })
    }

    /// The stored scene as text.
    pub fn stored(&self) -> String {
        String::from_utf8(self.scene.clone().expect("a stored scene")).unwrap()
    }
}

impl InProcessWorker {
    /// `CheckScene` of `json`, with `svgs` (name, text) and the generators `contribs` its fills may name.
    pub async fn check_scene(
        &mut self,
        json: &str,
        svgs: &[(&str, &str)],
        contribs: &[ContribEntry],
    ) -> SceneReply {
        let dir = self.session.join(IN);
        std::fs::write(dir.join("scene.json"), json).unwrap();
        let mut names = Vec::new();
        for (i, (name, text)) in svgs.iter().enumerate() {
            let file = format!("svg-{i}");
            std::fs::write(dir.join(&file), format!("{name}\n{text}")).unwrap();
            names.push(InputName::new(&file).unwrap());
        }
        std::fs::write(
            dir.join("contribs.json"),
            serde_json::to_vec(contribs).unwrap(),
        )
        .unwrap();
        let name = |n: &str| InputName::new(n).unwrap();
        let lines = self
            .request(|id| WorkerRequest::CheckScene {
                id,
                def: scene_draft_def(),
                scene: name("scene.json"),
                svgs: names,
                contribs: name("contribs.json"),
                measure: MeasureSpec::default(),
                check_only: false,
            })
            .await;
        SceneReply::of(lines)
    }

    /// Every reply line to frames of the stored scene `stored` drawn alone at `times`, with the generators `contribs`
    /// its fills may name handed over as `fills.json` (none when None), as the app sends them.
    pub async fn frames_scene_alone_lines(
        &mut self,
        stored: &str,
        contribs: Option<&[ContribEntry]>,
        times: &[f64],
    ) -> Vec<String> {
        let dir = self.session.join(IN);
        let contribs = contribs.map(|c| {
            std::fs::write(dir.join("fills.json"), serde_json::to_vec(c).unwrap()).unwrap();
            InputName::new("fills.json").unwrap()
        });
        let spec = FramesSpec {
            source: FrameSource::Alone {
                draft: Box::new(DraftSpec {
                    def: scene_draft_def(),
                    body: stored.into(),
                    params: Default::default(),
                }),
                contribs,
            },
            times: times.to_vec(),
            format: FormatKind::Landscape,
            long_edge: 320,
            sheet: false,
            busy: false,
        };
        self.request(|id| WorkerRequest::Frames { id, spec }).await
    }

    /// `project` (its videos drawn from `still`) with the contributions `contribs`, at `times`: the frames and the
    /// notes they said.
    pub async fn frames_project_with(
        &mut self,
        project: Project,
        contribs: &[ContribEntry],
        times: &[f64],
        still: image::RgbaImage,
    ) -> (Vec<FrameView>, Vec<String>) {
        let videos: Vec<String> = project
            .assets
            .iter()
            .filter(|a| a.kind == klipolo_model::project::AssetKind::Video)
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
        self.write_project(
            &project,
            contribs,
            &files,
            serde_json::json!({"videos": index}),
        );
        let frames = self
            .frames(super::project::project_source(), times, 512)
            .await;
        (frames, self.notes.clone())
    }
}
