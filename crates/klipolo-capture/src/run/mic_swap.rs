//! The mic's swaps while the take runs: to a fallback when its device is gone for 3 s, and back to the person's own
//! mic when that returns (a gamer's USB mic is better than the built-in one). Each swap is a
//! new run on the same source; the threads layer's controller looks for the person's own mic once a second while the
//! take records on another, and reports it with `DeviceBack`.

use super::core::SessionCore;
use super::effect::Effect;
use super::source::{Status, mic_rules};
use crate::backend::Transport;
use crate::english::{code, msg};
use crate::heal::HealAction;
use crate::manifest::source_param;
use crate::pipe::PipeEvent;
use crate::shell::{Flash, Level};

impl SessionCore {
    /// The mic's device is gone for 3 s: the built-in mic, else another connected mic that isn't virtual; with none,
    /// the mic waits for its own device (`DeviceBack` restarts it) and says it is lost, flashing the HUD for that at
    /// most once a minute (`HealPolicy::lost_flash`).
    pub(super) fn fall_back(&mut self, i: usize, now: u64, fx: &mut Vec<Effect>) {
        let src = &self.srcs[i];
        let current = src.device.clone().unwrap_or_default();
        let to = self.fallback_for(i);
        let id = src.id.clone();
        let Some(to) = to else {
            self.heal.fallback_unavailable(&id);
            let note = msg!(
                "capture.health.lost",
                source = source_param(&self.srcs[i].label),
                reason = "noFallback"
            );
            let flash = self.heal.lost_flash(&id, now).then_some(Flash::Problem);
            return self.note(note, Level::Warning, Some(&id), true, flash, fx);
        };
        let note = msg!(
            "capture.health.micFallback",
            device = self.mic_name(&current),
            fallback = to.name.as_str()
        );
        self.note(
            note,
            Level::Warning,
            Some(&id),
            true,
            Some(Flash::Problem),
            fx,
        );
        self.swap_to(i, to.id, now, fx);
    }

    /// The mic it would fall back to: the built-in one, else another connected mic that isn't virtual.
    fn fallback_for(&self, i: usize) -> Option<crate::backend::DeviceInfo> {
        let current = self.srcs[i].device.as_deref().unwrap_or_default();
        let usable = |m: &&crate::backend::DeviceInfo| m.id != current && !m.is_virtual;
        (self.mics.iter().filter(usable))
            .find(|m| m.transport == Transport::BuiltIn)
            .or_else(|| self.mics.iter().find(usable))
            .cloned()
    }

    /// The mic's own device flaps (back more than 3 times in 60 s). With a fallback: one mild note, then
    /// the fallback (the switch-back rules then apply). With none, it is never held (lost
    /// voice is worse than extra runs): it restarts now, as every time it returns, with a mild note at most once a
    /// minute.
    pub(super) fn flapping(&mut self, i: usize, now: u64, fx: &mut Vec<Effect>) {
        let (id, kind) = (self.srcs[i].id.clone(), self.srcs[i].kind);
        if self.fallback_for(i).is_none() {
            self.act(i, HealAction::Restart, now, fx);
            let said = self.heal.mic_dropping(&id, now);
            return self.act(i, said, now, fx);
        }
        // Through the pipe-note rule, so it is said once per take whoever notices the flapping.
        let note = PipeEvent::Note(code!("capture.note.micFlapping").into());
        let said = self.heal.decide(&id, kind, &note, now);
        self.act(i, said, now, fx);
        self.heal.on_fallback(&id);
        self.fall_back(i, now, fx);
    }

    /// The person's own mic is back after the fallback: the fallback's run ends and is made durable at once (its
    /// gathered sound appended, its levels and `RunEnded` journaled, then a flush tick), and the mic swaps back; its own
    /// device's first block opens the next run. Nothing the fallback recorded is lost: only the blocks already on their
    /// way while the swap is decided (a few ms) are left out, and the gap is the time the device takes to start.
    pub(super) fn switch_back(&mut self, i: usize, now: u64, fx: &mut Vec<Effect>) {
        let src = &self.srcs[i];
        let Some(own) = src
            .own
            .clone()
            .filter(|own| src.device.as_ref() != Some(own))
        else {
            return;
        };
        self.end_run(i, fx);
        fx.push(Effect::Tick);
        self.srcs[i].switch_from = self.srcs[i].device.clone();
        self.swap_to(i, own, now, fx);
    }

    /// A mic pipe started while the take runs: a switch back is done, and the person hears their own mic is back.
    pub(super) fn switched_back(&mut self, i: usize, fx: &mut Vec<Effect>) {
        if self.srcs[i].switch_from.take().is_none() {
            return;
        }
        let src = &self.srcs[i];
        let note = msg!(
            "capture.health.micBack",
            device = self.mic_name(src.device.as_deref().unwrap_or_default())
        );
        let id = src.id.clone();
        self.note(note, Level::Ok, Some(&id), true, None, fx);
    }

    /// The person's own mic didn't start after all (listed a moment before it can stream): the take goes straight
    /// back to the fallback, as a new run, rather than waiting 3 s with no voice; a later `DeviceBack` tries again.
    /// Returns false when no switch was under way.
    pub(super) fn back_to_fallback(&mut self, i: usize, now: u64, fx: &mut Vec<Effect>) -> bool {
        let Some(back) = self.srcs[i].switch_from.take() else {
            return false;
        };
        tracing::warn!(
            source = self.srcs[i].id,
            "core: the person's own mic didn't start; back on the fallback"
        );
        self.heal.on_fallback(&self.srcs[i].id);
        self.swap_to(i, back, now, fx);
        true
    }

    /// The mic records on `device` from its next start: a new run, with that device's rules.
    fn swap_to(&mut self, i: usize, device: String, now: u64, fx: &mut Vec<Effect>) {
        let src = &mut self.srcs[i];
        (src.device, src.status, src.restart_at) =
            (Some(device.clone()), Status::Starting, Some(now));
        src.mic_rules = Some(mic_rules(&src.id, Some(&device), &self.mics));
        fx.push(Effect::SwapMic {
            source: src.id.clone(),
            device,
        });
    }

    /// A mic's name from the device list, else its id.
    fn mic_name(&self, id: &str) -> String {
        (self.mics.iter().find(|m| m.id == id)).map_or_else(|| id.to_owned(), |m| m.name.clone())
    }
}
