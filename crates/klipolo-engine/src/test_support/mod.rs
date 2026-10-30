//! Helpers the engine's test binaries share (doc-hidden): a GPU device or a skip, the caption test project, one
//! frame drawn through the export worker's own start-up, and the caption frames (`captions`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use klipolo_model::apply_ops;
use klipolo_model::captions::sidecar::CaptionFile;
use klipolo_model::contrib::ContribId;
use klipolo_model::font::FontRef;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::secs_to_us as secs;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, TextStyle, Track, TrackKind};

use crate::api::{ExportEvent, ExportQuality};
use crate::export::job::ExportJob;
use crate::export::naming::plan_files;
use crate::export::runner::{Batch, Outcome};
use crate::export::worker::fonts_before_frame_zero;
use crate::plan::{PlanContext, frame_plan};
use crate::present::Output;
use crate::render::GpuBackend;
use crate::render::device::RenderDevice;
use crate::session::{PreviewBackend, RenderRequest, RenderedRgba};

mod caption_drawer;
mod captions;
mod scenes;
pub use caption_drawer::{FrameCosts, TestCaptionDrawer, caption_drawer, caption_drawer_at};
pub use captions::{
    caption_stats_via_export_pass, compare, render_caption_frame, render_caption_frame_via_export,
    render_caption_frames, sound_only_captions_project,
};
pub use scenes::export_frame_notes;
#[cfg(any(test, feature = "test-support"))]
pub use scenes::export_frame_notes_without_text_thread;

/// A device for a GPU test, or None (it prints "skipped: no GPU adapter") on a machine without one. Under
/// `KLIPOLO_REQUIRE_GPU=1` (CI's lavapipe lines) it panics instead, so a gate never passes a GPU test it skipped.
pub async fn adapter_or_skip() -> Option<Arc<RenderDevice>> {
    match RenderDevice::new().await {
        Ok(dev) => Some(dev),
        Err(e) if std::env::var("KLIPOLO_REQUIRE_GPU").as_deref() == Ok("1") => {
            panic!("no GPU adapter under KLIPOLO_REQUIRE_GPU=1: {e}")
        }
        Err(e) => {
            eprintln!("skipped: no GPU adapter ({e})");
            None
        }
    }
}

fn voice() -> Asset {
    Asset {
        id: "v".into(),
        kind: AssetKind::Video,
        path: "/media/v".into(),
        name: "v".into(),
        duration_us: secs(60.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
    }
}

/// The caption test project, the shape the model's tests use (`tests/common/speech.rs`): one 60 s clip of voice
/// file `v`, captions on in the built-in `style` (`caption.bold`) in every format, and a complete English transcript of
/// `words` (text, start s, end s).
pub fn captions_project(style: &str, words: &[(&str, f64, f64)]) -> (Project, Speech) {
    let mut p = Project::new("Pages");
    p.settings.formats = vec![
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ];
    p.assets = vec![voice()];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "v".into() },
        0,
        secs(60.0),
    )];
    p.captions.enabled = true;
    p.captions.style = Some(ContribId::builtin(style));
    let p = apply_ops(&p, &[]).expect("a checked project");
    let words = words
        .iter()
        .map(|&(text, start, end)| Word {
            text: text.into(),
            start_us: secs(start),
            end_us: secs(end),
            confidence: 0.9,
            kind: WordKind::Word,
        })
        .collect();
    let transcript = Transcript {
        key: "k".into(),
        language: "en".into(),
        model: "m".into(),
        covered_us: secs(60.0),
        complete: true,
        words,
    };
    let speech = Speech {
        transcripts: HashMap::from([("v".to_owned(), Arc::new(transcript))]),
        ..Speech::default()
    };
    (p, speech)
}

/// Frame 0 of a one-title export job (`text` in `font`, landscape), made with `ExportJob::create` in a temp folder and
/// drawn as the export worker draws it, without encoding: the worker's fonts first (`fonts_before_frame_zero`), then
/// its renderer (`GpuBackend::for_export_on`, whose compositor waits for the system font scan), on `dev`.
pub async fn render_text_via_export(
    dev: &Arc<RenderDevice>,
    text: &str,
    font: &FontRef,
) -> RenderedRgba {
    let dir = tempfile::tempdir().expect("a temp folder");
    let mut p = Project::new("Fonts");
    let mut titles = Track::new(TrackKind::Overlay, "Titles");
    titles.items = vec![Item {
        id: "title".into(),
        start_us: 0,
        duration_us: secs(1.0),
        attach: None,
        layout: Layout::new(
            Anchor::TopLeft,
            BoxN {
                x: 0.1,
                y: 0.3,
                w: 0.8,
                h: 0.4,
            },
        ),
        content: ItemContent::Text {
            text: text.into(),
            style: TextStyle {
                font: font.clone(),
                size: 120.0,
                ..TextStyle::default()
            },
        },
        effects: Vec::new(),
    }];
    p.timeline.tracks = vec![titles];
    let files = plan_files(
        &p,
        &[FormatKind::Landscape],
        ExportQuality::High,
        dir.path(),
    );
    let batch = Batch {
        project: p,
        fps: 30,
        quality: ExportQuality::High,
        files,
        pcm_dir: None,
        speech: Arc::default(),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    };
    let job = ExportJob::create(&batch, 10).expect("the export job");
    fonts_before_frame_zero(&job);
    let mut backend = GpuBackend::for_export_on(dev.clone());
    let ctx = PlanContext::new(Arc::new(job.project.clone()))
        .with_captions(job.captions.clone())
        .with_text_language(job.text_language.as_str().into());
    let plan = frame_plan(&ctx, 0, FormatKind::Landscape);
    backend
        .render(RenderRequest {
            plan,
            width: 640,
            height: 360,
            output: Output::Rgba,
        })
        .await
        .expect("frame 0")
        .finish()
        .await
        .expect("its pixels")
}

impl ExportJob {
    /// Tests: `create` for a one-file landscape batch of `project` and `speech` in `dir` (10 s segments).
    #[doc(hidden)]
    pub fn create_for_test(dir: &Path, project: &Project, speech: Arc<Speech>) -> Self {
        Self::create_for_test_with(dir, project, speech, None)
    }

    /// Tests: `create_for_test` asking for `caption_file` too.
    #[doc(hidden)]
    pub fn create_for_test_with(
        dir: &Path,
        project: &Project,
        speech: Arc<Speech>,
        caption_file: Option<CaptionFile>,
    ) -> Self {
        let files = plan_files(project, &[FormatKind::Landscape], ExportQuality::High, dir);
        let batch = Batch {
            project: project.clone(),
            fps: 30,
            quality: ExportQuality::High,
            files,
            pcm_dir: None,
            speech,
            text_language: "en".into(),
            contribs: Default::default(),
            caption_file,
            allow_stand_ins: false,
            activity_dir: None,
            allow_missing_cursor: false,
        };
        Self::create(&batch, 10).expect("the export job")
    }
}

/// Tests: `job` finished as a run finishes it, without encoding: each output's joined file stands in (a few bytes),
/// then `deliver::finish` puts the files in place. The paths delivered.
#[doc(hidden)]
pub fn finish_fake_export(job: ExportJob) -> Vec<PathBuf> {
    finish_fake_export_said(job).0
}

/// `finish_fake_export`, with what it said.
#[doc(hidden)]
pub fn finish_fake_export_said(mut job: ExportJob) -> (Vec<PathBuf>, Vec<ExportEvent>) {
    for o in &mut job.outputs {
        o.joined = true;
    }
    for o in &job.outputs {
        std::fs::write(job.path(&o.file), b"a joined video").expect("the joined file");
    }
    job.save().expect("the job");
    let said = Mutex::new(Vec::new());
    let outcome = crate::export::deliver::finish(&mut job, &|e| said.lock().unwrap().push(e));
    let delivered = match outcome {
        Outcome::Finished(paths) => paths,
        other => panic!("not finished: {other:?}"),
    };
    (delivered, said.into_inner().unwrap())
}
