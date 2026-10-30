//! LUT textures on the GPU and the bind groups the composite shader reads them through.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::upload::{CubeLutHalf, bind_group, upload};
use super::{lut_warning, parse_cube};
use crate::plan::LutRef;
use crate::vendor::cap::rendering::composite_frame::CompositeVideoFramePipeline;

/// LUT files kept as textures at once (a 65³ LUT is 2.2 MB of texels); the least recently used one goes first.
const MAX_LUTS: usize = 8;
/// How often a loaded LUT's file is looked at again (it may have been re-exported).
const RECHECK: Duration = Duration::from_secs(1);

type Texture = (wgpu::TextureView, u32);
type Done = Arc<Mutex<Option<Result<Texture, String>>>>;

struct Job {
    handle: JoinHandle<()>,
    /// The load's result, stored before the redraw signal fires (so the redraw always finds it).
    done: Done,
}

impl Job {
    fn take(&self) -> Option<Result<Texture, String>> {
        self.done
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

struct Loaded {
    modified: Option<SystemTime>,
    checked: Instant,
    /// `LutTextures::tick` when a layer last used it.
    used: u64,
    /// The texture view and LUT size; None before the first load finishes or when the file can't be used.
    texture: Option<Texture>,
    /// Reading, parsing and uploading the file on a blocking thread (the previous texture draws meanwhile).
    job: Option<Job>,
    /// The last load failed: the clip draws without its LUT and says so.
    failed: bool,
    /// Bind groups by strength (f32 bits), newest last.
    groups: Vec<(u32, wgpu::BindGroup)>,
}

/// LUT textures by path (reloaded when the file's modification time changes) and the bind groups that pair one with
/// a strength. Without a LUT, layers bind a 1×1×1 texture with the LUT switched off. Loading never blocks the
/// preview: a clip draws without its LUT until the file is parsed and uploaded, then the redraw signal fires; export
/// waits instead. A frame whose LUT is already loaded allocates nothing and doesn't touch the disk.
pub struct LutTextures {
    sampler: wgpu::Sampler,
    off: wgpu::BindGroup,
    loaded: HashMap<String, Loaded>,
    tick: u64,
    redraw: Arc<Notify>,
}

impl LutTextures {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &CompositeVideoFramePipeline,
        redraw: Arc<Notify>,
    ) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("LUT sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let dummy = upload(device, queue, &CubeLutHalf::identity_1());
        let off = bind_group(device, pipeline, &sampler, &dummy, [0.0; 4]);
        Self {
            sampler,
            off,
            loaded: HashMap::new(),
            tick: 0,
            redraw,
        }
    }

    /// Drops every LUT texture (the project closed or changed).
    pub fn forget(&mut self) {
        for (_, loaded) in self.loaded.drain() {
            if let Some(job) = loaded.job {
                job.handle.abort();
            }
        }
    }

    /// Under memory pressure: lets go of every LUT no layer used since `since` (a `frame_tick`) and stops its load.
    pub fn trim(&mut self, since: u64) {
        self.loaded.retain(|_, l| {
            let keep = l.used > since;
            if !keep && let Some(job) = l.job.take() {
                job.handle.abort();
            }
            keep
        });
    }

    /// The use counter now (a frame's start, for `trim`).
    pub fn frame_tick(&self) -> u64 {
        self.tick
    }

    /// Starts loading `lut` if it isn't loaded or loading (the playback lookahead calls this before the clip).
    pub fn warm(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, lut: &LutRef) {
        self.poll(device, queue, &lut.path);
    }

    /// The bind group for `lut`: the "off" group when there is none, while it loads (preview) or when it can't be
    /// used (plus a warning). With `wait` (export) a load in progress is waited for.
    pub async fn bind(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &CompositeVideoFramePipeline,
        lut: Option<&LutRef>,
        warnings: &mut Vec<String>,
        wait: bool,
    ) -> wgpu::BindGroup {
        let Some(lut) = lut else {
            return self.off.clone();
        };
        self.poll(device, queue, &lut.path);
        if wait && let Some(job) = self.loaded.get_mut(&*lut.path).and_then(|l| l.job.as_mut()) {
            let _ = (&mut job.handle).await;
            let done = job.take().unwrap_or_else(|| Err(lut_warning(&lut.path)));
            self.finish(&lut.path, done);
        }
        let Some(loaded) = self.loaded.get_mut(&*lut.path) else {
            return self.off.clone();
        };
        let Some((view, size)) = &loaded.texture else {
            if loaded.failed {
                warnings.push(lut_warning(&lut.path));
            }
            return self.off.clone();
        };
        let strength = lut.strength.clamp(0.0, 1.0) as f32;
        if let Some((_, group)) = loaded.groups.iter().find(|(s, _)| *s == strength.to_bits()) {
            return group.clone();
        }
        let params = [1.0, strength, *size as f32, 0.0];
        let group = bind_group(device, pipeline, &self.sampler, view, params);
        if loaded.groups.len() == 4 {
            loaded.groups.remove(0);
        }
        loaded.groups.push((strength.to_bits(), group.clone()));
        group
    }

    /// Rechecks the file (once a second), takes a finished load and starts one when the file is new or changed.
    fn poll(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path: &str) {
        self.tick += 1;
        let now = Instant::now();
        let start = match self.loaded.get_mut(path) {
            Some(l) => {
                l.used = self.tick;
                if let Some(done) = l.job.as_ref().and_then(Job::take) {
                    self.finish(path, done);
                    false
                } else if l.job.is_none() && now.duration_since(l.checked) >= RECHECK {
                    l.checked = now;
                    l.modified != modified(path)
                } else {
                    false
                }
            }
            None => {
                while self.loaded.len() >= MAX_LUTS {
                    let Some(oldest) = self
                        .loaded
                        .iter()
                        .min_by_key(|(_, l)| l.used)
                        .map(|(p, _)| p.clone())
                    else {
                        break;
                    };
                    if let Some(job) = self.loaded.remove(&oldest).and_then(|l| l.job) {
                        job.handle.abort();
                    }
                }
                self.loaded.insert(
                    path.to_string(),
                    Loaded {
                        modified: None,
                        checked: now,
                        used: self.tick,
                        texture: None,
                        job: None,
                        failed: false,
                        groups: Vec::with_capacity(4),
                    },
                );
                true
            }
        };
        if start && let Some(l) = self.loaded.get_mut(path) {
            l.modified = modified(path);
            let (device, queue, redraw) = (device.clone(), queue.clone(), self.redraw.clone());
            let owned = path.to_string();
            let done: Done = Arc::default();
            let result = done.clone();
            let handle = tokio::task::spawn_blocking(move || {
                let loaded = std::fs::read_to_string(&owned)
                    .map_err(|e| e.to_string())
                    .and_then(|text| parse_cube(&text))
                    .map(|lut| {
                        let half = CubeLutHalf::from(&lut);
                        (upload(&device, &queue, &half), half.size)
                    });
                *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(loaded);
                // After the result is stored: the redraw it triggers always finds it.
                redraw.notify_one();
            });
            l.job = Some(Job { handle, done });
        }
    }

    fn finish(&mut self, path: &str, done: Result<Texture, String>) {
        let Some(l) = self.loaded.get_mut(path) else {
            return;
        };
        l.job = None;
        match done {
            Ok(texture) => {
                l.texture = Some(texture);
                l.failed = false;
                l.groups.clear();
            }
            Err(error) => {
                let file = crate::log_name(path);
                tracing::warn!(%error, %file, "LUT not loaded");
                l.texture = None;
                l.failed = true;
                l.groups.clear();
            }
        }
    }
}

fn modified(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
