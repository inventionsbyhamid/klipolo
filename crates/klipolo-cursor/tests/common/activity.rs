//! Synthetic activity for the activity tests: 128 × 72 frames of a screen with a typed line, a caret, a counter and
//! a playing video; their ticks and file; a cursor resting or moving; ticks made of given regions; and two hours of a
//! tutorial's ticks.

use std::time::{Duration, Instant};

use klipolo_cursor::activity::{
    ActivityTrack, CHUNK_TICKS, Differ, GRID, Region, TICK_US, Tick, decode, encode_chunk, header,
};
use klipolo_cursor::facts::StillRuns;
use klipolo_cursor::format::Line;
use klipolo_cursor::{Rect01, Us};

use super::{Rng, file, head, line, read};

pub const S: Us = 1_000_000;
pub type Frame = [u8; 128 * 72];

/// What a synthetic screen shows over a grey background with ±2 of noise; frames are 100 ms apart. Every box sits on
/// whole grid cells (2 × 2 px).
#[derive(Clone, Copy, Default)]
pub struct Scene {
    /// A typed line 6 px tall at (8, 20), growing 2 px (one grid cell) rightwards each frame for this many frames.
    pub typed: u32,
    /// A caret: 2 × 2 px at (60, 40), toggling every 5 frames (2 Hz).
    pub caret: bool,
    /// A counter beside the caret: 4 × 4 px at (64, 38), toggling once a second, between the caret's toggles.
    pub counter: bool,
    /// A playing video: 12 × 8 px at (80, 40), re-randomised every frame for this many frames.
    pub video: u32,
}

impl Scene {
    pub fn frame(&self, k: u32, rng: &mut Rng, out: &mut Frame) {
        for p in out.iter_mut() {
            *p = 126 + (rng.next_u64() % 5) as u8;
        }
        let mut fill = |(x0, y0, w, h): (usize, usize, usize, usize), v: &mut dyn FnMut() -> u8| {
            for y in y0..y0 + h {
                for p in &mut out[y * 128 + x0..y * 128 + x0 + w] {
                    *p = v();
                }
            }
        };
        fill((8, 20, 2 * k.min(self.typed) as usize, 6), &mut || 30);
        if self.caret && (k / 5).is_multiple_of(2) {
            fill((60, 40, 2, 2), &mut || 0);
        }
        if self.counter && ((k + 3) / 10).is_multiple_of(2) {
            fill((64, 38, 4, 4), &mut || 255);
        }
        if k < self.video {
            fill((80, 40, 12, 8), &mut || rng.next_u64() as u8);
        }
    }
}

/// `n` ticks of `scene` through the differ (n + 1 frames: the first only primes it).
pub fn ticks_of(scene: &Scene, n: u32) -> Vec<Tick> {
    let mut differ = Differ::new();
    let mut rng = Rng::new(7);
    let mut frame = Box::new([0; 128 * 72]);
    let mut ticks = Vec::new();
    for k in 0..=n {
        scene.frame(k, &mut rng, &mut frame);
        ticks.extend(differ.push(&frame));
    }
    ticks
}

/// The activity file of `ticks`: its header, then a chunk a minute.
pub fn file_of(ticks: &[Tick]) -> Vec<u8> {
    let mut bytes = Vec::new();
    header(ticks.len() as Us * TICK_US, &mut bytes);
    for (i, chunk) in ticks.chunks(CHUNK_TICKS as usize).enumerate() {
        encode_chunk(i as u32, chunk, &mut bytes);
    }
    bytes
}

/// `ticks` through their file, read back.
pub fn activity_of(ticks: &[Tick]) -> ActivityTrack {
    decode(&file_of(ticks), ticks.len() as Us * TICK_US)
}

pub fn track_of(scene: &Scene, n: u32) -> ActivityTrack {
    activity_of(&ticks_of(scene, n))
}

/// `n` ticks that differ from their neighbours (`changed` counts 0–6 over and over).
pub fn numbered(n: u16) -> Vec<Tick> {
    let tick = |i: u16| Tick {
        changed: i % 7,
        ..Tick::default()
    };
    (0..n).map(tick).collect()
}

/// The cursor resting mid-screen for 30 s (a heartbeat every 500 ms).
pub fn resting() -> StillRuns {
    let mut lines = vec![head()];
    lines.extend((0..=60).map(|k| Line::Move {
        t: k * 500_000,
        x: 0.5,
        y: 0.5,
    }));
    StillRuns::of(&read(&file(&lines)))
}

/// The cursor moving across the screen for 30 s.
pub fn moving() -> StillRuns {
    StillRuns::of(&line(0.1, 0.9, 0.5, 30 * S))
}

/// The cursor moving across the screen until `rest_us`, then resting there for 30 s (the recording clock).
pub fn moving_then_resting(rest_us: Us) -> StillRuns {
    let steps = rest_us / 20_000;
    let x = |k: Us| 0.1 + 0.8 * k as f32 / steps as f32;
    let mut lines = vec![head()];
    lines.extend((0..=steps).map(|k| Line::Move {
        t: k * 20_000,
        x: x(k),
        y: 0.5,
    }));
    lines.extend((1..=60).map(|k| Line::Move {
        t: rest_us + k * 500_000,
        x: 0.9,
        y: 0.5,
    }));
    StillRuns::of(&read(&file(&lines)))
}

/// `rect` is the grid cells (x0, y0, w, h).
pub fn assert_cells(rect: Rect01, (x0, y0, w, h): (u8, u8, u8, u8)) {
    let (gw, gh) = (GRID.0 as f32, GRID.1 as f32);
    let want = Rect01 {
        x: f32::from(x0) / gw,
        y: f32::from(y0) / gh,
        w: f32::from(w) / gw,
        h: f32::from(h) / gh,
    };
    let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
    assert!(
        near(rect.x, want.x)
            && near(rect.y, want.y)
            && near(rect.w, want.w)
            && near(rect.h, want.h),
        "{rect:?} is not the cells {:?}",
        (x0, y0, w, h)
    );
}

/// Two hours of ticks as a screen tutorial gives them: seeded stretches of 5–60 s that are quiet, typed (a small region
/// moving right), playing a video (a steady box changing in 90 % of ticks) or scrolling (most of the screen).
pub fn two_hours() -> Vec<Tick> {
    let mut rng = Rng::new(5);
    let region = |x0: u8, y0: u8, w: u8, h: u8, cells: u16| Region {
        x0,
        y0,
        x1: x0 + w - 1,
        y1: y0 + h - 1,
        cells,
    };
    let mut ticks = Vec::with_capacity(72_000);
    while ticks.len() < 72_000 {
        let len = rng.range(50.0, 600.0) as usize;
        let kind = rng.next_u64() % 4;
        let (x, y) = (rng.range(0.0, 40.0) as u8, rng.range(0.0, 24.0) as u8);
        for k in 0..len {
            let mut t = Tick::default();
            let changed = match kind {
                0 => {
                    t.changed = (k % 3) as u16;
                    None
                }
                1 => k
                    .is_multiple_of(2)
                    .then(|| region(x + (k / 2 % 20) as u8, y, 1, 3, 3)),
                2 => (rng.unit() < 0.9).then(|| region(x, y, 20, 12, 200)),
                _ => Some(region(0, 4, 64, 30, 1_500)),
            };
            if let Some(r) = changed {
                t = Tick {
                    changed: r.cells,
                    top: [r, Region::default()],
                };
            }
            ticks.push(t);
        }
    }
    ticks.truncate(72_000);
    ticks
}

/// The best of five runs of `f` (the build server is shared, and its noise only adds).
pub fn best(f: &mut dyn FnMut()) -> Duration {
    let mut took = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        f();
        took = took.min(start.elapsed());
    }
    took
}

/// A tick of `regions`, the two largest first.
pub fn tick_of(regions: &[Region]) -> Tick {
    let mut top = [Region::default(); 2];
    for &r in regions {
        if r.cells > top[0].cells {
            top = [r, top[0]];
        } else if r.cells > top[1].cells {
            top[1] = r;
        }
    }
    let changed = regions.iter().map(|r| r.cells).sum();
    Tick { changed, top }
}

/// The cells (x0, y0, w, h), every one changed.
pub fn cells(x0: u8, y0: u8, w: u8, h: u8) -> Region {
    let (x1, y1) = (x0 + w - 1, y0 + h - 1);
    let cells = u16::from(w) * u16::from(h);
    Region {
        x0,
        y0,
        x1,
        y1,
        cells,
    }
}
