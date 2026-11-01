//! The core's rules over time: the once-a-second tick (the writer's flush, the heartbeat, the disk outlook, the
//! writer's queue, the stall and drop rules, the mic rules, the heal policy's waits, the read-back), the heal policy's
//! actions, pause and resume, and what the shell reports (focus, power, the game quitting, the parent going).
//!
//! Finer rules:
//! - The mic rules and the rate meter run on a clock that stops while the take is paused (`rules_clock`).
//! - The heal policy's timed checks (`device_still_missing`, `still_blank`) are polled once a second; the low-disk note
//!   is given when its level changes, not every tick.
//! - One stall reported by both the pipe and `VideoRules` spends one restart: a video source's restart under way
//!   absorbs further stall reports until its first frame, or for a first-frame grace (`FIRST_FRAME_GRACE_NS`, a slow
//!   camera). A stop, or anything from a sound source, is always acted on.
//! - A screen that stops while the display sleeps waits for the wake to restart, so a sleeping display never burns the
//!   restart budget.
//! - A lost mic falls back to the built-in one, else another connected mic; with none, it waits for its own device and
//!   restarts on it when it comes back (`DeviceBack`). After a fallback, the person's own mic
//!   coming back takes over again, as a new run (`mic_swap`).
//! - A gated mic's `micZeros` flashes the HUD at most once a minute.

use std::collections::BTreeMap;

use super::core::{Effect, PipeCmd, SessionCore};
use super::source::Status;
use crate::disk::{self, DiskLevel};
use crate::english::msg;
use crate::heal::{HealAction, SourceKind};
use crate::health::{VideoProblem, dbfs, peak_of};
use crate::media::{PcmBlock, SourceId};
use crate::pipe::{PipeEvent, VideoStats};
use crate::protocol::Event;
use crate::shell::{Flash, Level, WorkerState};
use crate::writer::{DiskFailure, STOP_QUEUED_MS, WARN_QUEUED_MS, WriterSnapshot};

/// A restarted pipe's first frame may take this long before the pipe counts as stalled (Continuity Camera, capture
/// cards: about 2-3 s).
pub const FIRST_FRAME_GRACE_NS: u64 = 3_000_000_000;
/// The HUD flashes for a gated mic's zeros at most this often.
const ZEROS_FLASH_NS: u64 = 60_000_000_000;
/// The countdown's meter, ten times a second.
const METER_NS: u64 = 100_000_000;

impl SessionCore {
    pub(super) fn tick_rules(
        &mut self,
        now: u64,
        writer: &WriterSnapshot,
        free: Option<u64>,
        stats: &BTreeMap<SourceId, VideoStats>,
        display_awake: bool,
        fx: &mut Vec<Effect>,
    ) {
        self.ticks += 1;
        self.display_awake = display_awake;
        fx.push(Effect::Emit(Event::Heartbeat {
            n: self.ticks,
            queued_ms: writer.queued_ms,
            free_bytes: free,
        }));
        let running = matches!(self.state, WorkerState::Recording | WorkerState::Paused);
        if running {
            // The gathered sound and cursor lines are appended for the tick to push (while paused: what arrived after
            // the pause began).
            self.flush_gathered(fx);
            self.cursor_tick(now, true, fx);
        }
        if running || self.state == WorkerState::Countdown {
            fx.push(Effect::Tick);
        }
        if running {
            // After the tick, so a running level never reaches the disk ahead of the media it covers.
            self.running_levels(fx);
        }
        if running {
            self.disk_and_queue(now, writer, free, fx);
        }
        self.wake_restarts(now, fx);
        for i in 0..self.srcs.len() {
            if self.state == WorkerState::Countdown {
                self.mic_check(i, now, fx);
            }
            if self.state != WorkerState::Recording {
                continue;
            }
            self.video_check(i, now, stats, fx);
            self.mic_check(i, now, fx);
            self.waits(i, now, fx);
        }
        if self.state == WorkerState::Recording
            && self
                .read_back_at
                .is_some_and(|at| self.clock.elapsed_us(now) >= at)
        {
            self.read_back_at = None;
            fx.push(Effect::ReadBack);
        }
        fx.push(self.status(now));
    }

    fn disk_and_queue(
        &mut self,
        now: u64,
        writer: &WriterSnapshot,
        free: Option<u64>,
        fx: &mut Vec<Effect>,
    ) {
        self.rate.add(
            self.rules_clock.elapsed_us(now) as u64 * 1_000,
            writer.bytes,
        );
        if let Some(free) = free {
            let rate = self.rate.bytes_per_s().unwrap_or(0.0);
            let outlook = disk::outlook(free, rate);
            self.minutes_left = (outlook.minutes_left != u32::MAX).then_some(outlook.minutes_left);
            if outlook.level == DiskLevel::Stop {
                return self.stop_and_save("capture.stop.disk", now, fx);
            }
            if outlook.level != self.disk {
                self.disk = outlook.level;
                if let Some(note) = outlook.note() {
                    self.health(note, false, Some(Flash::LowSpace), fx);
                }
            }
        }
        if let Some(failed) = writer.failed {
            let code = match failed {
                DiskFailure::Full => "capture.stop.disk",
                _ => "capture.stop.slowDrive",
            };
            return self.stop_and_save(code, now, fx);
        }
        if writer.queued_ms >= STOP_QUEUED_MS {
            return self.stop_and_save("capture.stop.slowDrive", now, fx);
        }
        if writer.queued_ms >= WARN_QUEUED_MS && !self.slow {
            self.slow = true;
            let note = msg!("capture.health.slowDrive");
            self.note(note, Level::Warning, None, true, Some(Flash::Problem), fx);
        } else if writer.queued_ms < WARN_QUEUED_MS / 2 {
            self.slow = false;
        }
    }

    /// A screen that stopped while the display slept restarts once the display is awake and the take records (at a
    /// tick, or at Resume after a wake during the pause), without spending the restart budget: the sleep stopped it,
    /// not a fault. The display's level is tested, never its edge.
    pub(super) fn wake_restarts(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if !self.display_awake || self.state != WorkerState::Recording {
            return;
        }
        for i in 0..self.srcs.len() {
            if std::mem::take(&mut self.srcs[i].wake_restart) {
                self.act(i, HealAction::Restart, now, fx);
            }
        }
    }

    /// The stall and drop rules for a live video source, and the keyframe ask after dropped packets.
    fn video_check(
        &mut self,
        i: usize,
        now: u64,
        stats: &BTreeMap<SourceId, VideoStats>,
        fx: &mut Vec<Effect>,
    ) {
        let src = &mut self.srcs[i];
        if !src.is_video() {
            return;
        }
        if src.status != Status::Live {
            return;
        }
        let s = stats.get(&src.id).copied().unwrap_or_default();
        src.fps_seen = s.delivered.saturating_sub(src.delivered_seen);
        src.delivered_seen = s.delivered;
        // The pipe's gate dropped packets since the last tick: ask for a keyframe now, so the wait for one is short
        // (the gate asks itself at most every 2 s, and its own forced keyframe may have been dropped too).
        if s.dropped_channel > src.dropped_seen {
            fx.push(self.pipe(i, PipeCmd::Keyframe));
        }
        let src = &mut self.srcs[i];
        src.dropped_seen = s.dropped_channel;
        if src
            .restart_at
            .is_some_and(|at| now.saturating_sub(at) < FIRST_FRAME_GRACE_NS)
        {
            return;
        }
        let awake = src.kind != SourceKind::Screen || self.display_awake;
        let problem = src
            .video_rules
            .as_mut()
            .and_then(|r| r.check(&s, now, awake));
        match problem {
            Some(VideoProblem::Stalled) => self.heal_event(i, &PipeEvent::Stalled, now, fx),
            Some(VideoProblem::Dropping(note)) => self.health(note, false, None, fx),
            None => {}
        }
        if let HealAction::Note(note) = self.heal.still_blank(&self.srcs[i].id, now) {
            self.health(note, true, Some(Flash::Problem), fx);
        }
    }

    /// The heal policy's wait for a lost device, once a second.
    fn waits(&mut self, i: usize, now: u64, fx: &mut Vec<Effect>) {
        let src = &self.srcs[i];
        if let Status::WaitingDevice(since) = src.status {
            let action = self
                .heal
                .device_still_missing(&src.id, src.kind, since, now);
            self.act(i, action, now, fx);
        }
    }

    /// A lost mic falls back the moment its 3 s are up ("within 3 s"), not at the next tick: one comparison per mic.
    pub(super) fn mic_fallback_due(&mut self, now: u64, fx: &mut Vec<Effect>) {
        for i in 0..self.srcs.len() {
            if let (SourceKind::Mic, Status::WaitingDevice(since)) =
                (self.srcs[i].kind, self.srcs[i].status)
                && now >= since + crate::heal::MIC_FALLBACK_NS
            {
                self.waits(i, now, fx);
            }
        }
    }

    pub(super) fn mic_check(&mut self, i: usize, now: u64, fx: &mut Vec<Effect>) {
        let at = self.rules_clock.elapsed_us(now);
        let src = &mut self.srcs[i];
        let Some(note) = src.mic_rules.as_mut().and_then(|r| r.check(at)) else {
            return;
        };
        let warning = note.level == Level::Warning;
        src.mic_warned |= warning;
        let mut flash = warning.then_some(Flash::Problem);
        if note.code == "capture.health.micZeros" {
            if src
                .zeros_flash_at
                .is_some_and(|t| now.saturating_sub(t) < ZEROS_FLASH_NS)
            {
                flash = None;
            } else {
                src.zeros_flash_at = Some(now);
            }
        }
        if self.state == WorkerState::Countdown && warning {
            // A mic rule firing in the countdown holds it, with the gate's question.
            let id = src.id.clone();
            self.countdown.held_by = Some(id.clone());
            self.countdown.held_note = Some(note.coded());
            fx.push(Effect::Emit(Event::Gate { missing: vec![id] }));
        }
        self.health(note, warning, flash, fx);
    }

    pub(super) fn feed_mic_rules(&mut self, i: usize, block: &PcmBlock) {
        let at = self.rules_clock.elapsed_us(block.host_ns);
        if let Some(rules) = &mut self.srcs[i].mic_rules {
            rules.feed(block, at);
        }
    }

    pub(super) fn pcm_in_countdown(
        &mut self,
        i: usize,
        block: &PcmBlock,
        now: u64,
        fx: &mut Vec<Effect>,
    ) {
        if self.srcs[i].status != Status::Live {
            return;
        }
        self.feed_mic_rules(i, block);
        let peak = peak_of(block);
        let src = &mut self.srcs[i];
        if let Some(sample) = crate::gate::GateSample::from_pcm(block, src.kind)
            && let Some(gate) = &mut self.gate
        {
            gate.observe(&src.id, sample);
        }
        if peak > 0.0 && self.countdown.held_by.as_deref() == Some(src.id.as_str()) {
            self.countdown.held_by = None;
        }
        src.meter.0 = src.meter.0.max(peak);
        if now.saturating_sub(src.meter.1) >= METER_NS {
            fx.push(Effect::Emit(Event::Meter {
                source: src.id.clone(),
                peak_dbfs: dbfs(src.meter.0),
            }));
            src.meter = (0.0, now);
        }
        self.mic_check(i, now, fx);
        self.advance(now, fx);
    }
}
