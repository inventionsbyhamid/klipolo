//! How the threads layer's results come back to the core: a writer's refusal, the read-back's findings, the end of
//! Stop's finishing, and the runs Stop finishes.

use std::collections::BTreeMap;

use klipolo_model::coded::CodedError;
use klipolo_model::time::Us;

use super::core::SessionCore;
use super::effect::{Effect, PipeCmd, RunDone};
use crate::protocol::Event;
use crate::session::FileKey;
use crate::shell::{Flash, Level, ShellOutput, WorkerState};
use crate::writer::{DiskFailure, WriterError};

impl SessionCore {
    /// The writer refused an effect's call: `Full` for a run's append ends that run (its next sample opens another, at
    /// a keyframe); `Gone` stops and saves (`failed`: why the writer stopped, from its snapshot).
    pub fn writer_refused(
        &mut self,
        key: &str,
        error: WriterError,
        failed: Option<DiskFailure>,
        now_ns: u64,
    ) -> Vec<Effect> {
        let mut fx = Vec::new();
        match error {
            WriterError::Gone => {
                let code = match failed {
                    Some(DiskFailure::Full) => "capture.stop.disk",
                    _ => "capture.stop.slowDrive",
                };
                self.stop_and_save(code, now_ns, &mut fx);
            }
            WriterError::Full => {
                if let Some(i) = self.srcs.iter().position(|s| s.run_key() == Some(key)) {
                    tracing::warn!(
                        key,
                        "core: the writer's queue was full, so the run ends here"
                    );
                    self.end_run(i, &mut fx);
                    if self.srcs[i].is_video() {
                        fx.push(self.pipe(i, PipeCmd::Keyframe));
                    }
                } else {
                    // A line file's lines or a cursor shape's image: dropped, never silently.
                    tracing::warn!(
                        key,
                        "core: the writer's queue was full, so this write was dropped"
                    );
                }
            }
        }
        fx
    }

    /// The read-back's findings (`readback::check`): a file that doesn't read back, or a mic file with less than a
    /// second of sound when the mic rules haven't already said so, gives one critical note.
    pub fn read_back(
        &mut self,
        problems: Vec<crate::readback::Problem>,
        now_ns: u64,
    ) -> Vec<Effect> {
        let mut fx = Vec::new();
        let real = problems.iter().any(|p| {
            !p.silent
                || !self
                    .index(&p.source)
                    .is_some_and(|i| self.srcs[i].mic_warned)
        });
        if real && !self.read_back_bad {
            self.read_back_bad = true;
            let note = crate::english::msg!("capture.health.readBack");
            self.note(
                note,
                Level::Critical,
                None,
                true,
                Some(Flash::Problem),
                &mut fx,
            );
            fx.push(self.status(now_ns));
        }
        fx
    }

    /// The threads layer finished the files after `Finalize`: Ok gives `Finished` and exit 0; an error (the drive
    /// stopped answering, a disk step failed) gives `Failed` and exit 1, and recovery finishes the take next launch.
    pub fn finished(&mut self, result: Result<(), CodedError>, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        let (state, code) = match result {
            Ok(()) => {
                let session = self.cfg.id.clone();
                fx.push(Effect::Emit(Event::Finished { session }));
                (WorkerState::Ready, 0)
            }
            Err(error) => {
                fx.push(Effect::Emit(Event::Failed { error }));
                (WorkerState::Failed, 1)
            }
        };
        self.state = state;
        self.emit_state(now_ns, &mut fx);
        fx.push(Effect::Shell(ShellOutput::Quit));
        fx.push(Effect::Exit(code));
        fx
    }

    /// Every run ended, and a resumed take's earlier runs' lengths: what Stop finishes (`run::finish`).
    pub fn take_done(&mut self) -> (Vec<RunDone>, BTreeMap<FileKey, Us>) {
        (
            std::mem::take(&mut self.done),
            std::mem::take(&mut self.earlier),
        )
    }
}
