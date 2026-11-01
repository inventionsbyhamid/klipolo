//! `FakeShell`: a worker shell without an OS. It sends its scripted inputs at their times (ms after `run` starts) and
//! hands every output it is shown to the test, until `Quit` or until the session drops its end.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use klipolo_platform::clock::host_ns;

use crate::shell::{Shell, ShellInput, ShellIo, ShellOutput};

pub struct FakeShell {
    inputs: Vec<(u32, ShellInput)>,
    outputs: Sender<ShellOutput>,
}

impl FakeShell {
    /// The shell, and where its outputs arrive (`Quit` included).
    pub fn new(inputs: Vec<(u32, ShellInput)>) -> (Self, Receiver<ShellOutput>) {
        let (outputs, rx) = channel();
        (Self { inputs, outputs }, rx)
    }
}

impl Shell for FakeShell {
    fn run(self: Box<Self>, io: ShellIo) {
        let Self {
            mut inputs,
            outputs,
        } = *self;
        inputs.sort_by_key(|(ms, _)| *ms);
        let start = host_ns();
        let mut inputs: VecDeque<_> = inputs
            .into_iter()
            .map(|(ms, input)| (start + u64::from(ms) * 1_000_000, input))
            .collect();
        loop {
            let now = host_ns();
            while inputs.front().is_some_and(|(at, _)| *at <= now) {
                let (_, input) = inputs.pop_front().expect("a due input");
                if io.input.send(input).is_err() {
                    return;
                }
            }
            let got = match inputs.front() {
                Some((at, _)) => io
                    .output
                    .recv_timeout(Duration::from_nanos(at.saturating_sub(now))),
                None => io.output.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match got {
                Ok(out) => {
                    let quit = out == ShellOutput::Quit;
                    let _ = outputs.send(out);
                    if quit {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}
