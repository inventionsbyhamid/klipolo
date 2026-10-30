//! A take's shape images made ready for the atlas: a PNG inside the take's own `cursors/` folder
//! (links followed), at most 256 × 256, decoded with `image` and premultiplied. The preview decodes on a thread of its
//! own and keeps at most `READY_MAX` decoded shapes waiting for `CursorPass::upload_ready` (≤ 4 MB); export and
//! headless renders decode inline (`decode`, from `preload`). A shape is never decoded on the render path in preview.

use std::collections::VecDeque;
use std::fmt;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use image::codecs::png::PngDecoder;
use image::{DynamicImage, ImageDecoder};
use tokio::sync::Notify;

/// The largest shape image, on either side.
pub const MAX_SIDE: u32 = 256;
/// Decoded shapes waiting for the next frame's upload, at most (each ≤ 256 KB).
pub const READY_MAX: usize = 16;

/// Why a shape image can't be used: it then draws Klipolo's Arrow look, in preview and export alike.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CursorShapeError {
    /// Not a regular file inside its take's `cursors/` folder (a link out of the take, a pipe, a device).
    OutsideTake,
    /// Larger than 256 × 256, or empty.
    TooLarge { width: u32, height: u32 },
    /// Missing, unreadable or not a PNG.
    Unreadable(String),
}

impl fmt::Display for CursorShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideTake => write!(f, "the cursor shape isn't inside its take"),
            Self::TooLarge { width, height } => {
                write!(
                    f,
                    "the cursor shape is {width} × {height}, over {MAX_SIDE} × {MAX_SIDE}"
                )
            }
            Self::Unreadable(why) => write!(f, "the cursor shape can't be read: {why}"),
        }
    }
}

impl std::error::Error for CursorShapeError {}

/// A decoded shape: premultiplied RGBA8, rows packed.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `png`, a shape of the take whose cursor file is `take`, decoded: refused unless it is a regular file inside the
/// take's `cursors/` folder (links followed) and at most 256 × 256 (checked from the header before decoding).
pub fn decode(take: &Path, png: &Path) -> Result<ShapeImage, CursorShapeError> {
    let unreadable = |e: &dyn fmt::Display| CursorShapeError::Unreadable(e.to_string());
    let dir = take.parent().filter(|d| !d.as_os_str().is_empty());
    let root = dir
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|e| unreadable(&e))?
        .join("cursors");
    let real = png.canonicalize().map_err(|e| unreadable(&e))?;
    let checked = std::fs::metadata(&real).ok().filter(|m| m.is_file());
    let Some(checked) = checked.filter(|_| real.starts_with(&root)) else {
        return Err(CursorShapeError::OutsideTake);
    };
    // The file opened is the one checked: a swap between the check and the open is refused, and a
    // pipe swapped in can't hold the open up.
    let opened = crate::media::cursor::open_regular(&real).map_err(|e| unreadable(&e))?;
    let Some((file, _)) = opened.filter(|(_, m)| same_file(&checked, m)) else {
        return Err(CursorShapeError::OutsideTake);
    };
    let decoder = PngDecoder::new(BufReader::new(file)).map_err(|e| unreadable(&e))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(CursorShapeError::TooLarge { width, height });
    }
    let mut rgba = DynamicImage::from_decoder(decoder)
        .map_err(|e| unreadable(&e))?
        .into_rgba8()
        .into_raw();
    // Premultiplied, as the atlas's mips and the pass's blending expect (once per shape, off the render path).
    for px in rgba.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
    Ok(ShapeImage {
        width,
        height,
        rgba,
    })
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

/// Windows has no stable file id in std: the containment check before the open stands alone there.
#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    true
}

/// A shape decoded (or refused) on the decoder's thread.
pub struct Decoded {
    pub take: Arc<str>,
    pub shape: u16,
    pub image: Result<ShapeImage, CursorShapeError>,
}

/// The preview's decoder: a queue of shapes to decode, a thread that runs while there are any, and the decoded shapes
/// waiting for the next frame (at most `READY_MAX`; the thread stops when that many wait and the next take resumes it).
pub struct Decoder {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<Queue>,
    /// Fired when a shape is ready: the preview draws again with it.
    redraw: Arc<Notify>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<(Arc<str>, u16, Arc<str>)>,
    ready: VecDeque<Decoded>,
    worker: bool,
    /// Bumped when the atlas is let go: a decode that finishes after it is dropped.
    generation: u64,
}

fn lock(m: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Decoder {
    pub fn new(redraw: Arc<Notify>) -> Self {
        let state = Queue {
            jobs: VecDeque::with_capacity(READY_MAX),
            ready: VecDeque::with_capacity(READY_MAX),
            ..Queue::default()
        };
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(state),
                redraw,
            }),
        }
    }

    /// Queues `png` (shape `shape` of take `take`) for decoding.
    pub fn submit(&self, take: Arc<str>, shape: u16, png: Arc<str>) {
        let mut q = lock(&self.shared.state);
        q.jobs.push_back((take, shape, png));
        start(&self.shared, &mut q);
    }

    /// The shapes decoded since the last call, oldest first (at most `READY_MAX`), into `out`.
    pub fn take_ready(&self, out: &mut Vec<Decoded>) {
        let mut q = lock(&self.shared.state);
        out.extend(q.ready.drain(..));
        start(&self.shared, &mut q);
    }

    /// Drops what is queued or waiting; a decode under way is dropped when it ends.
    pub fn forget(&self) {
        let mut q = lock(&self.shared.state);
        q.jobs.clear();
        q.ready.clear();
        q.generation = q.generation.wrapping_add(1);
    }
}

/// Starts the thread unless it runs, nothing is queued, or `READY_MAX` shapes wait already.
fn start(shared: &Arc<Shared>, q: &mut Queue) {
    if q.worker || q.jobs.is_empty() || q.ready.len() >= READY_MAX {
        return;
    }
    q.worker = true;
    let shared = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("klipolo-cursor-shapes".into())
        .spawn(move || work(&shared));
    if let Err(error) = spawned {
        q.worker = false;
        tracing::warn!(%error, "cursor shape thread");
    }
}

fn work(shared: &Shared) {
    // A frame is drawing the Arrow in its place meanwhile.
    crate::qos::set_current(crate::qos::Qos::UserInitiated);
    loop {
        let (job, generation) = {
            let mut q = lock(&shared.state);
            match (q.ready.len() < READY_MAX)
                .then(|| q.jobs.pop_front())
                .flatten()
            {
                Some(job) => (job, q.generation),
                None => {
                    q.worker = false;
                    return;
                }
            }
        };
        let (take, shape, png) = job;
        let image = decode(Path::new(&*take), Path::new(&*png));
        if let Err(error) = &image {
            tracing::warn!(file = %crate::log_name(&*png), %error, "cursor shape: drawn as the arrow");
        }
        let mut q = lock(&shared.state);
        if q.generation == generation {
            q.ready.push_back(Decoded { take, shape, image });
            drop(q);
            // After the shape is queued: the redraw it triggers always finds it.
            shared.redraw.notify_one();
        }
    }
}
