//! How the caption drawer asks for pages and keeps what comes back: the face a layer draws in, a page's key, the builds
//! asked of the build thread (`worker`) and the replies kept in the drawer's atlases. Nothing here builds a page.

use std::sync::Arc;

use klipolo_model::captions::Pages;
use klipolo_model::font::FontRef;

use super::cache::{Entry, Key, hash_of, words_hash};
use super::drawer::CaptionDrawer;
use super::worker::{Built, Gone, Reply, Request};
use crate::plan::CaptionLayer;
use crate::render::fonts::{self, FaceChoice, family_for};
use crate::vendor::cap::rendering::layers::fonts_scanned;

/// The face the captions draw in, resolved once per change of what it is resolved from.
pub(super) struct Face {
    pub(super) font: Arc<FontRef>,
    pub(super) weight: u16,
    pub(super) italic: bool,
    pub(super) generation: u64,
    pub(super) choice: FaceChoice,
    pub(super) hash: u64,
}

impl CaptionDrawer {
    /// The face `layer` draws in, resolved when what it is resolved from changed.
    pub(super) fn resolve_face(&mut self, layer: &CaptionLayer) {
        let generation = fonts::generation();
        let (weight, italic) = (layer.style.font.weight, layer.style.font.italic);
        let same = self.face.as_ref().is_some_and(|f| {
            f.generation == generation
                && f.weight == weight
                && f.italic == italic
                && (Arc::ptr_eq(&f.font, &layer.font) || f.font == layer.font)
        });
        if !same {
            let choice = family_for(&fonts::index(), &layer.font, weight, italic);
            let hash = hash_of((
                &choice.family,
                choice.weight,
                choice.italic,
                choice.stretch,
                choice.no_file,
            ));
            self.face = Some(Face {
                font: layer.font.clone(),
                weight,
                italic,
                generation,
                choice,
                hash,
            });
        }
    }

    /// The key of page `page` of `pages` (the format's, held by `layer`) in an output of `out` px.
    pub(super) fn key(
        &mut self,
        layer: &CaptionLayer,
        pages: &Arc<Pages>,
        page: u32,
        out: (u32, u32),
    ) -> Key {
        let words = match self
            .words
            .iter()
            .flatten()
            .find(|(p, at, _)| *at == page && Arc::ptr_eq(p, pages))
        {
            Some(&(_, _, hash)) => hash,
            None => {
                let hash = words_hash(pages, page as usize);
                self.words.rotate_right(1);
                self.words[0] = Some((pages.clone(), page, hash));
                hash
            }
        };
        Key {
            style: layer.style_key,
            face: self.face.as_ref().map_or(0, |f| f.hash),
            format: layer.format,
            out,
            max_width_px: (layer.max_width * f64::from(out.0)).round() as u32,
            words,
            language: hash_of(&*layer.language),
            generation: fonts::generation(),
            scanned: fonts_scanned(),
        }
    }

    /// Asks the build thread for page `page` under `key`, unless it is kept, already asked, or two builds are in flight.
    pub(super) fn want(
        &mut self,
        layer: &CaptionLayer,
        pages: &Arc<Pages>,
        page: u32,
        out: (u32, u32),
        key: Key,
    ) {
        let asked = self
            .in_flight
            .iter()
            .any(|&(k, e)| k == key && e == self.epoch);
        if self.gone || asked || self.in_flight.is_full() || self.cache.find(&key).is_some() {
            return;
        }
        let Some(face) = &self.face else {
            return;
        };
        let request = Request {
            key,
            epoch: self.epoch,
            style: layer.style.clone(),
            face: face.choice.clone(),
            language: layer.language.clone(),
            pages: pages.clone(),
            page,
            format: layer.format,
            out,
            max_width: layer.max_width,
        };
        match self.worker.send(request) {
            Ok(()) => self.in_flight.push((key, self.epoch)),
            Err(Gone) => self.gone = true,
        }
    }

    /// Picks up the build thread's replies: those there now, or (`block`) the next one.
    pub(super) fn drain(&mut self, device: &wgpu::Device, block: bool) {
        loop {
            let reply = if block {
                self.worker.reply().map(Some)
            } else {
                self.worker.try_reply()
            };
            match reply {
                Ok(Some(Reply::Pipe(pipe))) => self.pipe = Some(pipe),
                Ok(Some(Reply::Built(built))) => self.keep(device, built),
                Ok(None) => return,
                Err(Gone) => {
                    self.gone = true;
                    return;
                }
            }
            if block {
                return;
            }
        }
    }

    /// Keeps a built page, unless the fonts changed since it was asked (or it is kept already): then the build thread
    /// frees it, as it frees every atlas.
    pub(super) fn keep(&mut self, device: &wgpu::Device, built: Box<Built>) {
        let asked = (built.key, built.epoch);
        if let Some(i) = self.in_flight.iter().position(|&k| k == asked) {
            self.in_flight.remove(i);
        }
        let pipe = match &self.pipe {
            Some(pipe) if built.epoch == self.epoch && self.cache.find(&built.key).is_none() => {
                pipe
            }
            _ => return self.worker.unkept(built),
        };
        let bind = pipe.bind(device, &built.atlas.view, &self.buffers.view);
        self.builds += 1;
        let Built {
            key,
            layout,
            atlas,
            cjk_missing,
            ..
        } = *built;
        self.cache.0.push(Entry {
            key,
            layout,
            atlas,
            bind,
            cjk_missing,
            used: self.tick,
        });
    }
}
