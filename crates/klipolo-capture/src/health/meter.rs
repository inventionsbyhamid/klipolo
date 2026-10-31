//! The HUD's mic meter (`MicRules::level`).

use klipolo_model::time::Us;

/// A mic's recent loudest sample, the HUD's meter: the peaks of the current half-second window and the one before,
/// from the block peaks `MicRules::feed` already reads (nothing more per sample).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Meter {
    from_us: Us,
    now: f32,
    last: f32,
}

impl Meter {
    const WINDOW_US: Us = 500_000;

    pub(super) fn feed(&mut self, peak: f32, at_us: Us) {
        let gone = at_us - self.from_us;
        if gone >= Self::WINDOW_US {
            self.last = if gone < 2 * Self::WINDOW_US {
                self.now
            } else {
                0.0
            };
            (self.now, self.from_us) = (0.0, at_us);
        }
        self.now = self.now.max(peak.min(1.0));
    }

    pub(super) fn level(&self, now_us: Us) -> f32 {
        if now_us - self.from_us >= 2 * Self::WINDOW_US {
            0.0
        } else {
            self.now.max(self.last)
        }
    }
}
