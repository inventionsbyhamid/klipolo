//! A synthetic screen for the activity pass's tests: no decoder, one picture painted per 100 ms tick, so chunks, stops,
//! holds and cancels are counted exactly. The store opens it through its `Opener`, as it opens the platform's decoder.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use klipolo_engine::platform::activity::{ActivityError, ActivityFrames, LUMA_BYTES, Opener};
use klipolo_model::time::Us;

pub const TICK: Us = 100_000;

/// What the synthetic decoders did: where each was opened, the pictures they gave, and how many are open now.
#[derive(Default)]
pub struct Log {
    pub opened_at: Mutex<Vec<Us>>,
    pub frames: AtomicUsize,
    pub open_now: AtomicUsize,
}

impl Log {
    pub fn opened_at(&self) -> Vec<Us> {
        self.opened_at.lock().unwrap().clone()
    }

    pub fn frames(&self) -> usize {
        self.frames.load(Ordering::SeqCst)
    }

    pub fn open_now(&self) -> usize {
        self.open_now.load(Ordering::SeqCst)
    }
}

/// A screen `duration_us` long whose pictures each cost `per_frame` to "decode".
#[derive(Clone, Copy)]
pub struct Screen {
    pub duration_us: Us,
    pub per_frame: Duration,
}

struct Frames {
    screen: Screen,
    next: Us,
    log: Arc<Log>,
    cancel: Arc<AtomicBool>,
    /// Pictures in `[from, to)` can't be read (damaged data).
    bad: (Us, Us),
}

impl ActivityFrames for Frames {
    fn next(&mut self, out: &mut [u8; LUMA_BYTES]) -> Result<Option<Us>, ActivityError> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err(ActivityError::Cancelled);
        }
        if self.next >= self.screen.duration_us {
            return Ok(None);
        }
        if (self.bad.0..self.bad.1).contains(&self.next) {
            return Err(ActivityError::Decode("damaged data".into()));
        }
        paint(self.next / TICK, out);
        if !self.screen.per_frame.is_zero() {
            std::thread::sleep(self.screen.per_frame);
        }
        self.log.frames.fetch_add(1, Ordering::SeqCst);
        let at = self.next;
        self.next += TICK;
        Ok(Some(at))
    }

    fn duration_us(&self) -> Us {
        self.screen.duration_us
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.log.open_now.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A dark screen with a bright 8 × 8 block a cell further right each tick, a row lower each minute: every tick
/// changes, and every minute differs.
pub fn paint(tick: i64, out: &mut [u8; LUMA_BYTES]) {
    out.fill(30);
    let x = (tick as usize * 2) % 120;
    let y = 8 + (tick as usize / 600) % 56;
    for row in y..y + 8 {
        out[row * 128 + x..row * 128 + x + 8].fill(220);
    }
}

/// The store's decoder for `screen`, logging into `log`.
pub fn opener(screen: Screen, log: Arc<Log>) -> Opener {
    damaged_opener(screen, log, (0, 0))
}

/// `opener`, but the pictures in `[bad.0, bad.1)` can't be read.
pub fn damaged_opener(screen: Screen, log: Arc<Log>, bad: (Us, Us)) -> Opener {
    Arc::new(move |_path: &Path, from_us: Us, cancel: &Arc<AtomicBool>| {
        log.opened_at.lock().unwrap().push(from_us);
        log.open_now.fetch_add(1, Ordering::SeqCst);
        let frames = Frames {
            screen,
            next: from_us,
            log: log.clone(),
            cancel: cancel.clone(),
            bad,
        };
        Ok::<_, ActivityError>(Box::new(frames) as Box<dyn ActivityFrames>)
    })
}

/// A file that opens, `duration_us` long, but never gives a picture (every packet undecodable).
struct Blank(Us);

impl ActivityFrames for Blank {
    fn next(&mut self, _out: &mut [u8; LUMA_BYTES]) -> Result<Option<Us>, ActivityError> {
        Ok(None)
    }

    fn duration_us(&self) -> Us {
        self.0
    }
}

/// The store's decoder for a file that never gives a picture, counting its opens.
pub fn blank_opener(duration_us: Us, opens: Arc<AtomicUsize>) -> Opener {
    Arc::new(
        move |_path: &Path, _from_us: Us, _cancel: &Arc<AtomicBool>| {
            opens.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ActivityError>(Box::new(Blank(duration_us)) as Box<dyn ActivityFrames>)
        },
    )
}

/// A screen on a drive that goes at `gone_at`: its pictures from then on can't be read, and no decoder opens there
/// any more (`Unavailable`). Each open tried is logged, the failed ones too.
pub fn vanishing_opener(screen: Screen, log: Arc<Log>, gone_at: Us) -> Opener {
    let opens = damaged_opener(screen, log.clone(), (gone_at, Us::MAX));
    Arc::new(move |path: &Path, from_us: Us, cancel: &Arc<AtomicBool>| {
        if from_us >= gone_at {
            log.opened_at.lock().unwrap().push(from_us);
            return Err(ActivityError::Unavailable("the drive is gone".into()));
        }
        opens(path, from_us, cancel)
    })
}

/// A file that can't be read now (an OS error): every open fails as `Unavailable`, counted.
pub fn unavailable_opener(opens: Arc<AtomicUsize>) -> Opener {
    Arc::new(
        move |_path: &Path, _from_us: Us, _cancel: &Arc<AtomicBool>| {
            opens.fetch_add(1, Ordering::SeqCst);
            Err::<Box<dyn ActivityFrames>, _>(ActivityError::Unavailable(
                "permission denied".into(),
            ))
        },
    )
}
