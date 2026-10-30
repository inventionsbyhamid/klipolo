//! A caption drawer over a synthetic two-minute project, for the engine's tests (doc-hidden): pages drawn
//! the export's way (waiting for their build) or the preview's (never waiting), and the render thread's time per frame.

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_model::captions::{Captions, captions};
use klipolo_model::contrib::ContribId;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::speech::{Speech, Word, WordKind};
use klipolo_model::time::secs_to_us as secs;

use super::captions::sound_only_captions_project;
use crate::plan::captions::caption_layer;
use crate::plan::{CaptionLayer, PlanContext};
use crate::render::captions::CaptionDrawer;
use crate::render::device::RenderDevice;

/// A `CaptionDrawer` (its own build thread) on a test device, over 40 pages of Bold (one three-word sentence every 3 s,
/// each page's words its own), drawn for a portrait output of `out` px.
pub struct TestCaptionDrawer {
    dev: Arc<RenderDevice>,
    drawer: CaptionDrawer,
    project: Project,
    speech: Speech,
    captions: Arc<Captions>,
    out: (u32, u32),
    /// Frames are submitted without `after_submit` (`hold_staging`).
    holding: bool,
}

/// What a page costs the render thread (`time_frames`), in the preview's way.
#[derive(Clone, Copy, Debug)]
pub struct FrameCosts {
    /// The caption state (`caption_layer`), median µs.
    pub state_us: f64,
    /// The drawer's `prepare`, median and slowest µs over the frames (the page kept).
    pub prepare_us: f64,
    pub slowest_us: f64,
    /// The first frame's `prepare` (the page asked of the build thread), µs, and how long until it showed, ms.
    pub first_us: f64,
    pub shown_after_ms: f64,
}

/// See `TestCaptionDrawer` (1080 × 1920).
pub async fn caption_drawer() -> TestCaptionDrawer {
    caption_drawer_at((1080, 1920)).await
}

/// See `TestCaptionDrawer`.
pub async fn caption_drawer_at(out: (u32, u32)) -> TestCaptionDrawer {
    crate::render::fonts::configure_for_tests();
    let dev = RenderDevice::new().await.expect("a GPU");
    let (mut project, mut speech) = sound_only_captions_project("caption.bold", &[]);
    let words: Vec<Word> = (0..40)
        .flat_map(|page| {
            let at = f64::from(page) * 3.0;
            ["one", "two", "three."]
                .into_iter()
                .enumerate()
                .map(move |(k, word)| Word {
                    text: format!("{word}{page}").replace('.', "") + if k == 2 { "." } else { "" },
                    start_us: secs(at + k as f64 * 0.4),
                    end_us: secs(at + k as f64 * 0.4 + 0.3),
                    confidence: 0.9,
                    kind: WordKind::Word,
                })
        })
        .collect();
    let transcript = Arc::make_mut(speech.transcripts.get_mut("v").expect("its voice"));
    transcript.words = words;
    project.timeline.main[0].source_out_us = secs(120.0);
    project.assets[0].duration_us = secs(120.0);
    let captions = Arc::new(captions(&project, &speech));
    TestCaptionDrawer {
        drawer: CaptionDrawer::new(&dev, None, false),
        dev,
        project,
        speech,
        captions,
        out,
        holding: false,
    }
}

impl TestCaptionDrawer {
    fn context(&self, style: &str) -> PlanContext {
        let mut p = self.project.clone();
        p.captions.style = Some(ContribId::builtin(style));
        PlanContext::new(Arc::new(p)).with_captions(self.captions.clone())
    }

    /// The portrait caption layer of page `page` (at its middle) in the built-in `style`.
    fn layer(&self, style: &str, page: usize) -> CaptionLayer {
        let pages = &self.captions.pages[&FormatKind::Portrait];
        assert_eq!(pages.pages.len(), 40, "40 pages");
        let pg = &pages.pages[page];
        let at = pg.start_us + (pg.end_us - pg.start_us) / 2;
        caption_layer(
            &self.context(style),
            at,
            FormatKind::Portrait,
            &mut Vec::new(),
        )
        .expect("a caption")
    }

    /// `prepare` as the export (`wait`: then the GPU's work is waited for too) or the preview calls it, its uploads
    /// submitted as a frame's are: whether the page is drawn.
    fn prepare(&mut self, layer: &CaptionLayer, wait: bool) -> bool {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Test captions"),
        });
        let drawn = self
            .drawer
            .prepare(
                device,
                queue,
                &mut encoder,
                layer,
                self.out,
                &mut Vec::new(),
                wait,
            )
            .is_some();
        queue.submit([encoder.finish()]);
        if !self.holding {
            self.drawer.after_submit();
        }
        if wait {
            let _ = device.poll(wgpu::PollType::Wait);
        }
        drawn
    }

    /// Waits for the GPU (and the staging buffers' mapping, as the device's poll thread does each frame).
    pub fn settle(&self) {
        let _ = self.dev.device.poll(wgpu::PollType::Wait);
    }

    /// While `on`, frames are submitted without `after_submit`, as a frame path that missed it did:
    /// the staging buffers they copied from aren't mapped again, so past the kept ones uploads fall back to queue
    /// writes. Turned off, those buffers are mapped again.
    pub fn hold_staging(&mut self, on: bool) {
        if self.holding && !on {
            self.drawer.after_submit();
        }
        self.holding = on;
    }

    /// Uploads made with a queue write, a staging buffer of wgpu's own.
    pub fn queue_writes(&self) -> u64 {
        self.drawer.stats().queue_writes
    }

    /// Draws page `page` once, the export's way: it waits for the page's build.
    pub async fn draw_page(&mut self, style: &str, page: usize) {
        let layer = self.layer(style, page);
        assert!(self.prepare(&layer, true), "the page is drawn");
    }

    /// Prepares page `page` once, the preview's way: never waits. Whether it is drawn.
    pub fn preview_page(&mut self, style: &str, page: usize) -> bool {
        let layer = self.layer(style, page);
        self.prepare(&layer, false)
    }

    /// Draws page `page` of a new revision whose words didn't change (the captions dragged elsewhere): its pages are
    /// made anew, so nothing is shared with the last revision's but their content.
    pub async fn draw_page_in_new_revision(&mut self, style: &str, page: usize) {
        self.project
            .captions
            .place
            .insert(FormatKind::Portrait, 0.3);
        self.captions = Arc::new(captions(&self.project, &self.speech));
        let layer = self.layer(style, page);
        assert!(self.prepare(&layer, true), "the page is drawn");
    }

    /// The render thread's cost of page `page` in `style`, the preview's way (never waiting): the first frame asks the
    /// build thread for it, frames are prepared about every millisecond until it shows, then `frames` frames 1 ms of
    /// the timeline apart (the page kept, the next page built ahead on the build thread).
    pub fn time_frames(&mut self, style: &str, page: usize, frames: u32) -> FrameCosts {
        let ctx = self.context(style);
        let from = self.captions.pages[&FormatKind::Portrait].pages[page].start_us;
        let at = |t| caption_layer(&ctx, t, FormatKind::Portrait, &mut Vec::new());
        let first = at(from).expect("a caption");
        let started = Instant::now();
        let mut shown = self.prepare(&first, false);
        let first_us = started.elapsed().as_secs_f64() * 1e6;
        while !shown {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "the page shows"
            );
            std::thread::sleep(Duration::from_millis(1));
            shown = self.prepare(&first, false);
        }
        let shown_after_ms = started.elapsed().as_secs_f64() * 1e3;
        let (mut state, mut prepare) = (Vec::new(), Vec::new());
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        for k in 0..frames {
            let started = Instant::now();
            let layer = at(from + i64::from(k) * 1_000);
            state.push(started.elapsed().as_secs_f64() * 1e6);
            let Some(layer) = layer else { continue };
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Test captions"),
            });
            let started = Instant::now();
            std::hint::black_box(self.drawer.prepare(
                device,
                queue,
                &mut encoder,
                &layer,
                self.out,
                &mut Vec::new(),
                false,
            ));
            prepare.push(started.elapsed().as_secs_f64() * 1e6);
            queue.submit([encoder.finish()]);
            self.drawer.after_submit();
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(0.0)
        };
        FrameCosts {
            state_us: median(&mut state),
            prepare_us: median(&mut prepare),
            slowest_us: prepare.last().copied().unwrap_or(0.0),
            first_us,
            shown_after_ms,
        }
    }

    pub fn atlases(&self) -> usize {
        self.drawer.atlases()
    }

    pub fn atlas_bytes(&self) -> u64 {
        self.drawer.atlas_bytes()
    }

    pub fn builds(&self) -> u64 {
        self.drawer.builds()
    }

    /// The drawer's uploads of its frame buffers so far.
    pub fn uploads(&self) -> u64 {
        self.drawer.stats().uploads
    }

    /// Prepares page `page` in `style` at `share` of its time, the preview's way (never waits): whether it is drawn.
    pub fn preview_at(&mut self, style: &str, page: usize, share: f64) -> bool {
        let pg = &self.captions.pages[&FormatKind::Portrait].pages[page];
        let at = pg.start_us + ((pg.end_us - pg.start_us) as f64 * share) as i64;
        let layer = caption_layer(
            &self.context(style),
            at,
            FormatKind::Portrait,
            &mut Vec::new(),
        )
        .expect("a caption");
        self.prepare(&layer, false)
    }
}
