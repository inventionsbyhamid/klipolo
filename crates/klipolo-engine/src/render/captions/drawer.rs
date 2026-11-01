//! The caption drawer: a frame's `Layer::Caption` drawn from its
//! page's layout and atlas, which it keeps. Pages are built on the caption build thread (`worker`), never here.
//!
//! - **Atlases kept by content:** keyed by the style's mask hash, the face, the format, the output size, the line
//!   width, a hash of the page's words (text, size step, emphasis, hiding, new lines, keywords), the text language and
//!   the fonts' generation; never by pointers, so a new revision whose page didn't change (a trim elsewhere, a caption
//!   drag, a colour) finds the atlas it has. Per format at most `PER_FORMAT` atlases and `FORMAT_BYTES` bytes (evicted
//!   least recently used, by count and by bytes: a 4K page is about 4× a 1080p one), never the page drawn nor the
//!   next. Their bytes count toward the text ceiling (`TextDrawer::reserve`).
//! - **Builds off the render thread:** a page that isn't kept is asked of the build thread, and so is the next
//!   page that shows (prefetch), at most `MAX_IN_FLIGHT` at a time. Replies are picked up at the start of each
//!   `prepare`; the redraw signal fires when one lands. The preview draws nothing for a page until an atlas of it is
//!   there, never a stall; while a page's atlas is built again under a new key (a hovered style or font, a resize), it
//!   draws the page's last one, never another page's words. An export (`wait`) waits for the reply on the
//!   same path, so preview == export holds. The asking
//!   and keeping is `builds`.
//! - **Nor freed on the render thread:** an atlas evicted, every one after a font change and the drawer's own when it
//!   is dropped go back to the build thread, which frees them in order with its builds. Freeing a texture's view can
//!   wait for the GPU's queue (lavapipe takes the queue's lock for it, and holds that lock while it runs a submission,
//!   a page build's included), so a frame never frees one.
//! - **Per frame:** the face is resolved only when the font, weight, style or fonts' generation change; the words' hash
//!   is kept per page; the instances go into kept buffers (`draw`), uploaded only when they changed.
//!   Nothing is allocated once warm.

use std::sync::Arc;

use arrayvec::ArrayVec;
use klipolo_model::captions::Pages;
use klipolo_model::font::FontSource;
use tokio::sync::Notify;

use super::builds::Face;
use super::cache::{Atlases, Key};
pub use super::cache::{FORMAT_BYTES, PER_FORMAT};
use super::draw::{self, Buffers, DrawPipe, Instance, View};
use super::worker::Worker;
use crate::api::{FRAME_CJK_FONT_MISSING, FRAME_FONT_MISSING};
use crate::plan::CaptionLayer;
use crate::render::device::RenderDevice;
use crate::vendor::cap::rendering::layers::fonts_scanned;

/// Builds asked of the build thread at once: the page drawn and the next.
pub const MAX_IN_FLIGHT: usize = 2;
/// What a frame says when the build thread is gone (an export stops on any frame note: never a caption missing).
const NOT_DRAWN: &str = "The captions couldn't be drawn.";

/// See [`CaptionDrawer::stats`].
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptionStats {
    /// Preview frames drawn before their page's atlas was there (with the page's last atlas, or without captions).
    pub late: u64,
    /// Pages built, the atlases kept and their bytes.
    pub builds: u64,
    pub atlases: usize,
    pub atlas_bytes: u64,
    /// Uploads of the frame's quads and uniform (only when they changed), and the frames that made them through a
    /// staging buffer of their own (made and freed for that frame, every kept one in flight).
    pub uploads: u64,
    pub one_off_uploads: u64,
}

pub struct CaptionDrawer {
    pub(super) worker: Worker,
    /// The draw pipeline, the build thread's first reply.
    pub(super) pipe: Option<DrawPipe>,
    /// The build thread is gone: nothing more can be built.
    pub(super) gone: bool,
    pub(super) cache: Atlases,
    /// The builds asked and not answered yet, with the font epoch they were asked in.
    pub(super) in_flight: ArrayVec<(Key, u64), MAX_IN_FLIGHT>,
    /// Bumped by `fonts_changed`: a reply asked before it is dropped.
    pub(super) epoch: u64,
    pub(super) tick: u64,
    pub(super) face: Option<Face>,
    /// The words' hash of the last pages and page asked (the page drawn and the next), the pages held so their address
    /// can't be reused by others.
    pub(super) words: [Option<(Arc<Pages>, u32, u64)>; 2],
    pub(super) instances: Vec<Instance>,
    pub(super) view: View,
    pub(super) buffers: Buffers,
    /// This frame's: the entry drawn and its instances.
    pub(super) drawn: Option<(usize, u32)>,
    pub(super) builds: u64,
    /// Preview frames whose page's atlas wasn't there yet (drawn with the page's last one, or without captions): the
    /// perf harness's late frames at page changes.
    pub(super) late: u64,
}

impl CaptionDrawer {
    /// A drawer on `dev`, its build thread started (`wait`: an export's, whose font system waits for the system font
    /// scan); `redraw` fires when the thread has made the pipeline and after each page it builds.
    pub fn new(dev: &Arc<RenderDevice>, redraw: Option<Arc<Notify>>, wait: bool) -> Self {
        Self {
            worker: Worker::start(dev, redraw, wait),
            pipe: None,
            gone: false,
            cache: Atlases::default(),
            in_flight: ArrayVec::new(),
            epoch: 0,
            tick: 0,
            face: None,
            words: [None, None],
            instances: Vec::new(),
            view: View::default(),
            buffers: Buffers::new(&dev.device),
            drawn: None,
            builds: 0,
            late: 0,
        }
    }

    /// The atlases kept, their bytes, and how many pages were built (tests, and the text ceiling).
    pub fn atlases(&self) -> usize {
        self.cache.0.len()
    }

    pub fn atlas_bytes(&self) -> u64 {
        self.cache.bytes()
    }

    pub fn builds(&self) -> u64 {
        self.builds
    }

    /// What the captions cost and hold (tests and the perf harness).
    #[doc(hidden)]
    pub fn stats(&self) -> CaptionStats {
        CaptionStats {
            late: self.late,
            builds: self.builds,
            atlases: self.cache.0.len(),
            atlas_bytes: self.cache.bytes(),
            uploads: self.buffers.uploads,
            one_off_uploads: self.buffers.one_off_uploads,
        }
    }

    /// The frame `prepare` uploaded into is submitted: the staging buffers it copied from are mapped again.
    /// Every frame path calls it after its submit (`Compositor::frame_submitted`); one that didn't would keep them
    /// from being reused, and past the kept ones each upload would make a staging buffer of its own.
    pub fn after_submit(&mut self) {
        self.buffers.after_submit();
    }

    /// The frame `prepare` uploaded into never reached the GPU (it failed validation, or was given up before its
    /// submit): the next frame uploads its quads and uniform again rather than trust what the buffers hold.
    pub fn forget_uploads(&mut self) {
        self.buffers.forget_uploads();
    }

    /// The shared fonts changed (`SharedFonts::refresh` returned true): every atlas (freed on the build thread) and the
    /// face go, and a build asked before is dropped when it lands (the build thread refreshes its own fonts before each
    /// build).
    pub fn fonts_changed(&mut self) {
        self.worker.free(std::mem::take(&mut self.cache.0));
        self.face = None;
        self.epoch += 1;
    }

    /// Prepares `layer` for an output of `out` px: the build thread's replies are kept, its page and the next page
    /// that shows are asked for if they aren't kept, and the frame's instances are uploaded. The preview (`!wait`)
    /// draws the page's last atlas while it is being built again, else nothing; an export waits for it. Returns the slot `draw` takes, or None when
    /// there is nothing to draw. Says `frame.fontMissing` for a stand-in face and `frame.cjkFontMissing` for CJK without
    /// its Noto Sans, as titles do. It takes no queue, so it never waits for the GPU's queue, where a page build's work
    /// runs: only an export waits, for its page's reply.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        layer: &CaptionLayer,
        out: (u32, u32),
        warnings: &mut Vec<String>,
        wait: bool,
    ) -> Option<usize> {
        self.drawn = None;
        let pages = layer.captions.pages.get(&layer.format)?.clone();
        self.drain(device, false);
        while wait && self.pipe.is_none() && !self.gone {
            self.drain(device, true);
        }
        self.resolve_face(layer);
        self.tick += 1;
        let key = self.key(layer, &pages, layer.page, out);
        // The next page that shows (one of hidden words only takes no time).
        let next = (layer.page as usize + 1..pages.pages.len())
            .find(|&i| pages.pages[i].end_us > pages.pages[i].start_us)
            .map(|i| i as u32);
        let next_key = next.map(|n| (n, self.key(layer, &pages, n, out)));
        self.want(layer, &pages, layer.page, out, key);
        if let Some((n, k)) = next_key {
            self.want(layer, &pages, n, out, k);
        }
        while wait && self.cache.find(&key).is_none() && !self.gone {
            self.want(layer, &pages, layer.page, out, key);
            self.drain(device, true);
        }
        if self.gone {
            if !warnings.iter().any(|w| w == NOT_DRAWN) {
                warnings.push(NOT_DRAWN.into());
            }
            return None;
        }
        // The preview while this page's atlas builds under a new key (another style, face or size in the middle of a
        // continuous change): the same page's last atlas, never blank frames. An export waited for its own.
        let shown = match self.cache.find(&key) {
            Some(_) => key,
            None if !wait => {
                self.late += 1;
                self.cache.same_page(&key)?
            }
            None => return None,
        };
        let mut evicted = Vec::new();
        self.cache.evict(
            layer.format,
            [Some(shown), next_key.map(|(_, k)| k)],
            &mut evicted,
        );
        self.worker.free(evicted);
        let index = self.cache.find(&shown)?;
        let entry = &mut self.cache.0[index];
        entry.used = self.tick;
        let scanning = matches!(layer.font.source, FontSource::Installed) && !fonts_scanned();
        let stand_in = self.face.as_ref().is_some_and(|f| f.choice.stand_in) && !scanning;
        for (said, code) in [
            (stand_in, FRAME_FONT_MISSING),
            (entry.cjk_missing, FRAME_CJK_FONT_MISSING),
        ] {
            if said && !warnings.iter().any(|w| w == code) {
                warnings.push(code.into());
            }
        }
        let (layout, size) = (&entry.layout, entry.atlas.size);
        draw::instances(
            layer,
            layout,
            size,
            out,
            &mut self.instances,
            &mut self.view,
        );
        if self.instances.is_empty() {
            return None;
        }
        self.buffers
            .write(device, encoder, &self.instances, &self.view);
        self.drawn = Some((index, self.instances.len() as u32));
        Some(index)
    }

    /// Draws the captions `prepare` returned `slot` for, into `pass`: one instanced draw in the groups' order.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, slot: usize) {
        let (Some(pipe), Some((index, n)), Some(buffer)) =
            (&self.pipe, self.drawn, self.buffers.instances())
        else {
            return;
        };
        let Some(entry) = self.cache.0.get(index).filter(|_| index == slot) else {
            return;
        };
        pass.set_pipeline(&pipe.pipeline);
        pass.set_bind_group(0, &entry.bind, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..4, 0..n);
    }
}

impl Drop for CaptionDrawer {
    /// The atlases kept go to the build thread to free, as every other one does; the thread ends once it has them.
    fn drop(&mut self) {
        self.worker.free(std::mem::take(&mut self.cache.0));
    }
}
