//! The core's pipe inputs: a pipe that started (or failed to), and its media: encoded packets and PCM blocks (the
//! cursor's events are `cursor_in`'s). Before the gate they only feed it; while recording they are written (`admit`), and while paused a sample
//! stamped before the pause still is. At Record, the main picture not starting means no take; any other source that
//! didn't start is noted, and the take goes on without it.

use klipolo_model::coded::CodedError;

use super::core::{SessionCore, Started};
use super::effect::{Effect, PipeCmd};
use super::source::{Status, main_picture};
use crate::english::msg;
use crate::heal::SourceKind;
use crate::manifest::source_param;
use crate::media::{EncodedPacket, PcmBlock};
use crate::pipe::{PipeError, PipeEvent};
use crate::protocol::Event;
use crate::shell::{Level, WorkerState};

impl SessionCore {
    pub fn started(&mut self, source: &str, started: Started, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        let Some(i) = self.index(source) else {
            return fx;
        };
        if matches!(self.srcs[i].status, Status::Left | Status::Stopped)
            || matches!(self.state, WorkerState::Stopping | WorkerState::Ready)
        {
            fx.push(self.pipe(i, PipeCmd::Stop));
            return fx;
        }
        let src = &mut self.srcs[i];
        src.format = Some(started);
        src.status = Status::Live;
        match self.state {
            WorkerState::Starting => self.maybe_count_down(now_ns, &mut fx),
            WorkerState::Recording | WorkerState::Paused => {
                self.switched_back(i, &mut fx);
                self.live_again(i, now_ns, &mut fx);
            }
            _ => {}
        }
        fx
    }

    pub fn failed_to_start(&mut self, source: &str, error: PipeError, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        let Some(i) = self.index(source) else {
            return fx;
        };
        match self.state {
            WorkerState::Starting | WorkerState::Countdown => {
                // A source the person chose to record without ("Record without it") never ends the take as its start
                // fails after.
                let left = self.srcs[i].status == Status::Left;
                self.srcs[i].status = Status::Failed;
                if !left && main_picture(&self.srcs) == Some(i) {
                    // No take without its main picture: the other pipes stop and the empty
                    // folder goes; `Failed` says why.
                    let error = picture_error(self.srcs[i].kind, &error);
                    fx.push(Effect::Emit(Event::Failed { error }));
                    self.end_without_a_take(now_ns, &mut fx);
                    return fx;
                }
                let id = self.srcs[i].id.clone();
                // Named, so the countdown and the chat say which source didn't start.
                let note = msg!(
                    "capture.note.didntStart",
                    source = source_param(&self.srcs[i].label),
                    why = error.coded()
                );
                self.note(note, Level::Critical, Some(&id), true, None, &mut fx);
                if self
                    .srcs
                    .iter()
                    .all(|s| matches!(s.status, Status::Failed | Status::Left))
                {
                    fx.push(Effect::Emit(Event::Failed {
                        error: error.coded(),
                    }));
                    self.end_without_a_take(now_ns, &mut fx);
                } else {
                    // Its note says why, so the start gate never asks about it again: it
                    // leaves the gate at once, and the countdown goes on with the rest.
                    self.leave_gate(&id);
                    self.maybe_count_down(now_ns, &mut fx);
                    self.advance(now_ns, &mut fx);
                }
            }
            WorkerState::Recording | WorkerState::Paused => {
                // The person's own mic not starting after a fallback leaves the take on the fallback.
                if self.back_to_fallback(i, now_ns, &mut fx) {
                    return fx;
                }
                // A restart or a mic swap that didn't come up: the heal policy decides, as for the pipe's own event. A
                // mic that didn't start, whatever the error, waits for its device (or falls back) with its controller
                // looking for it, rather than spending restarts in a row and being given up.
                let mic = self.srcs[i].kind == SourceKind::Mic;
                let event = match error {
                    PipeError::PermissionDenied => PipeEvent::PermissionRevoked,
                    PipeError::NoDevice => PipeEvent::DeviceLost,
                    _ if mic => PipeEvent::DeviceLost,
                    _ => PipeEvent::StoppedBySystem,
                };
                self.srcs[i].restart_at = None;
                self.heal_event(i, &event, now_ns, &mut fx);
            }
            _ => {}
        }
        fx
    }

    pub fn packet(&mut self, source: &str, packet: EncodedPacket, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        let Some(i) = self.index(source) else {
            return fx;
        };
        match self.state {
            WorkerState::Countdown => {
                if self.srcs[i].status == Status::Live && !self.srcs[i].blank {
                    let id = &self.srcs[i].id;
                    if let Some(gate) = &mut self.gate {
                        gate.observe(id, crate::gate::GateSample::Frame);
                    }
                    self.advance(now_ns, &mut fx);
                }
            }
            // While paused too: a packet stamped before the pause keeps its time; the clock drops one stamped inside it.
            WorkerState::Recording | WorkerState::Paused => self.write_packet(i, &packet, &mut fx),
            _ => {}
        }
        fx
    }

    pub fn pcm(&mut self, source: &str, block: PcmBlock, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        let Some(i) = self.index(source) else {
            return fx;
        };
        match self.state {
            WorkerState::Countdown => self.pcm_in_countdown(i, &block, now_ns, &mut fx),
            WorkerState::Recording => {
                self.feed_mic_rules(i, &block);
                self.write_pcm(i, &block, &mut fx);
            }
            // The block a pause interrupted is written up to the pause (cut at its own frames), so the run goes on
            // across the pause with no step; the mic rules wait for the resume.
            WorkerState::Paused => self.write_pcm(i, &block, &mut fx),
            _ => {}
        }
        fx
    }
}

/// Why a take whose main picture didn't start records nothing: a refused permission as the recorder's own step for it
/// (the screen's, or the camera's in Camera only), else the pipe's own error.
fn picture_error(kind: SourceKind, error: &PipeError) -> CodedError {
    match (error, kind) {
        (PipeError::PermissionDenied, SourceKind::Camera) => msg!("capture.permission.camera"),
        (PipeError::PermissionDenied, _) => msg!("capture.permission.screen"),
        _ => error.coded(),
    }
}
