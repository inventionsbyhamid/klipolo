//! `CheckScene` and a scene folder's `Verify`: the scene checked
//! (`scene_check::check`), measured (`scene_check::scene_cost`), drawn as a 256 × 144 thumbnail at the middle of its
//! time on a line of its own (after waiting once for this computer's fonts), and answered
//! `Compiled` with the normalised bytes (`scene`) and the key over them. A refused scene answers its problems as
//! diagnostics with their JSON paths. Every key this worker checked within class is remembered for the session
//! (`Checked`), so frames of a project placing that draft draw it and a draft it never checked draws nothing.

use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

use klipolo_model::contrib::ContribDef;

use super::compile::thumbnail;
use super::{failed, send};
use crate::headless::Headless;
use crate::make::budget::CostClass;
use crate::make::draft::DraftLayer;
use crate::make::measure::{Deadline, Meter};
use crate::make::protocol::{Blob, Key, MeasureSpec, Step, WorkerReply};
use crate::make::scene_check::{CheckedScene, Fills, SceneInput, check, scene_cost};
use crate::registry::guard::Diagnostic;

/// Scene keys a worker remembers having checked, at most (the drafts' own cap is 64 in all).
pub const MAX_CHECKED: usize = 64;
/// How long a thumbnail or an agent's frames wait for this computer's fonts at most (the scan runs once, in the
/// background).
pub(super) const FONTS_WAIT: Duration = Duration::from_secs(3);

/// The scene keys this worker checked within class this session, the oldest going first past [`MAX_CHECKED`].
#[derive(Default)]
pub(crate) struct Checked(VecDeque<Key>);

impl Checked {
    pub fn insert(&mut self, key: Key) {
        if !self.0.contains(&key) {
            self.0.push_back(key);
        }
        while self.0.len() > MAX_CHECKED {
            self.0.pop_front();
        }
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.0.contains(key)
    }
}

/// One scene request.
pub(super) struct SceneJob<'a> {
    pub id: u64,
    pub def: &'a ContribDef,
    pub input: SceneInput<'a>,
    pub fills: Fills,
    pub spec: &'a MeasureSpec,
    pub deadline: Deadline,
}

/// `job` checked, measured and thumbnailed; its key remembered in `checked` when it is within class.
pub(super) async fn check_scene(
    meter: &mut dyn Meter,
    h: &mut Headless,
    job: SceneJob<'_>,
    out: &mut dyn Write,
    checked: &mut Checked,
) -> WorkerReply {
    let id = job.id;
    let progress = |out: &mut dyn Write, step| {
        let _ = send(out, &WorkerReply::Progress { id, step }, step);
    };
    progress(out, Step::Parse);
    let c = match check(job.def, job.input, job.fills) {
        Ok(c) => c,
        Err(problems) => return refused(id, &problems),
    };
    progress(out, Step::Measure);
    let cost = match scene_cost(meter, h, &c, job.spec, job.deadline).await {
        Ok(cost) => cost,
        Err(e) => return failed(id, Step::Measure, &e),
    };
    progress(out, Step::Frames);
    let mut diagnostics = Vec::new();
    if !fonts_ready(FONTS_WAIT).await {
        diagnostics.push(Diagnostic::info(
            "scene.fontsLoading",
            "This computer's fonts were still loading for the thumbnail: a text in an installed family may show its \
             stand-in there. That family isn't missing.",
        ));
    }
    match thumbnail(h, &alone(&c, job.spec)).await {
        Ok(jpeg) => {
            let _ = send(out, &WorkerReply::Thumbnail { id, jpeg }, Step::Frames);
        }
        Err(e) => return failed(id, Step::Frames, &e),
    }
    if cost.class != CostClass::Over {
        checked.insert(c.key);
    }
    let scene = match Blob::new(c.bytes) {
        Ok(scene) => scene,
        Err(e) => {
            return failed(
                id,
                Step::Parse,
                &format!("the scene is too large to send back: {e}"),
            );
        }
    };
    WorkerReply::Compiled {
        id,
        ok: true,
        diagnostics,
        facts: None,
        cost: Some(cost),
        resolved: None,
        key: Some(c.key),
        hardened: None,
        scene: Some(scene),
    }
}

/// `input` checked and normalised only (`CheckScene`'s `check_only`): no GPU, no cost, no thumbnail.
pub(super) fn checked_only(
    id: u64,
    def: &ContribDef,
    input: SceneInput<'_>,
    fills: Fills,
) -> WorkerReply {
    let c = match check(def, input, fills) {
        Ok(c) => c,
        Err(problems) => return refused(id, &problems),
    };
    let scene = match Blob::new(c.bytes) {
        Ok(scene) => scene,
        Err(e) => {
            return failed(
                id,
                Step::Parse,
                &format!("the scene is too large to send back: {e}"),
            );
        }
    };
    WorkerReply::Compiled {
        id,
        ok: true,
        diagnostics: Vec::new(),
        facts: None,
        cost: None,
        resolved: None,
        key: Some(c.key),
        hardened: None,
        scene: Some(scene),
    }
}

/// The scene at the request's settings, as its thumbnail draws it.
fn alone(c: &CheckedScene, spec: &MeasureSpec) -> DraftLayer {
    DraftLayer::of_scene(
        &c.def,
        c.scene.clone(),
        c.registry.clone(),
        spec.params.clone(),
        klipolo_model::project::FormatKind::Landscape,
    )
}

/// The scene's problems, each with its JSON path.
fn refused(id: u64, problems: &[klipolo_model::scene::SceneProblem]) -> WorkerReply {
    WorkerReply::Compiled {
        id,
        ok: false,
        diagnostics: problems.iter().map(Diagnostic::of_scene).collect(),
        facts: None,
        cost: None,
        resolved: None,
        key: None,
        hardened: None,
        scene: None,
    }
}

/// Whether this computer's fonts are scanned, waiting up to `limit` for the background scan.
pub(super) async fn fonts_ready(limit: Duration) -> bool {
    let started = Instant::now();
    loop {
        if crate::vendor::cap::rendering::layers::fonts_scanned() {
            return true;
        }
        if started.elapsed() >= limit {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
