//! The parity test's synthetic screens: 60 s of a line typed while the cursor rests, and a file that never
//! gives a picture.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use klipolo_engine::platform::activity::{ActivityError, ActivityFrames, LUMA_BYTES, Opener};
use klipolo_model::time::Us;

const TICK: Us = 100_000;
/// Each screen's length.
pub const LEN: Us = 60_000_000;

/// A 60 s screen, still but for a line typed from 20 s to 35 s while the cursor rests: a glyph a tick (a stroke a cell
/// wide and three tall) moving right along a row.
struct Typing {
    next: Us,
    cancel: Arc<AtomicBool>,
}

impl ActivityFrames for Typing {
    fn next(&mut self, out: &mut [u8; LUMA_BYTES]) -> Result<Option<Us>, ActivityError> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err(ActivityError::Cancelled);
        }
        if self.next >= LEN {
            return Ok(None);
        }
        out.fill(40);
        let tick = self.next / TICK;
        if (200..350).contains(&tick) {
            let x = 40 + 2 * ((tick - 200) % 30) as usize;
            for row in 50..56 {
                out[row * 128 + x..row * 128 + x + 2].fill(230);
            }
        }
        let at = self.next;
        self.next += TICK;
        Ok(Some(at))
    }

    fn duration_us(&self) -> Us {
        LEN
    }
}

/// A file that opens but never gives a picture: it can't be analysed.
struct Blank;

impl ActivityFrames for Blank {
    fn next(&mut self, _out: &mut [u8; LUMA_BYTES]) -> Result<Option<Us>, ActivityError> {
        Ok(None)
    }

    fn duration_us(&self) -> Us {
        LEN
    }
}

/// Typing screens, but `blank` never gives a picture; `opens` counts the decoders opened.
pub fn opener(blank: String, opens: Arc<AtomicUsize>) -> Opener {
    Arc::new(move |path: &Path, from_us: Us, cancel: &Arc<AtomicBool>| {
        opens.fetch_add(1, Ordering::SeqCst);
        let frames: Box<dyn ActivityFrames> = match path.to_string_lossy() == blank {
            true => Box::new(Blank),
            false => Box::new(Typing {
                next: from_us,
                cancel: cancel.clone(),
            }),
        };
        Ok(frames)
    })
}
