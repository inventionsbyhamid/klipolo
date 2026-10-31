//! The worker's protocol with the app (the app's supervisor): JSON lines, the app's commands on the worker's stdin and
//! the worker's events on its stdout. A line that doesn't parse is logged and skipped, never a Stop. The parent going
//! away (stdin's end of file) is never a Stop either: the worker records on, orphaned.
//!
//! Exit codes of `klipolo --record-worker` (`worker::main`): 0 the take is finished (`Finished` came first); 1 it failed,
//! and what reached the disk is recovered on the next launch; 2 the arguments or the build are wrong; 3 there is no take
//! (stopped or cancelled before the start gate, nothing written).

use klipolo_model::coded::CodedError;
use klipolo_model::time::Us;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::live_view_size::ViewPixels;
use crate::media::SourceId;
use crate::shell::{Level, WorkerState};

/// What the app asks. None of these reaches chat or MCP. The UI sends them through
/// the app's `recorder_send` (`Start` only on the person's own Record press).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, rename = "CaptureCommand")]
pub enum Command {
    /// The person's Record press (or "Start from my game"'s key press, which the shell sends as a hotkey).
    Start,
    Pause,
    Resume,
    Marker,
    /// Stop and save. Idempotent.
    Stop,
    /// "Cancel" on the countdown or the start gate's question: no take. After the start it saves, as Stop does.
    Cancel,
    /// "Record without it": the start gate stops waiting for `source`, which isn't recorded.
    WithoutSource {
        source: SourceId,
    },
    /// "Cancel" while armed and not started: armed for "Start from my game", or armed for Record before `Start` (the
    /// person cancels while it arms). The worker ends with no take: its hotkeys unregistered, its empty folder removed,
    /// exit 3 (the app then tells the UI `State Idle`).
    Disarm,
    /// Start over's next take: `Start`, sent by the app only after the person's own Start over press in the
    /// last take (its menu-bar item or key). The editor stays hidden, so the countdown and its question show on the HUD
    /// and the start key answers it, as in a take the start key began.
    StartAgain,
    /// The recorder's live view's picture follows the view that shows it: its size in device pixels, or
    /// None for the small view (`live_view_size`). The app's own to an open live view; a take ignores it, and the UI
    /// can't send it (`recorder_send` refuses it).
    ViewSize {
        view: Option<ViewPixels>,
    },
}

/// What the worker tells the app. The app forwards each to the UI as a `recorder-event` (a `Preview` frame as raw bytes
/// over the UI's channel: `preview_wire`), and words its own supervisor's outcomes the same way (`Failed` with
/// `capture.stop.crashLoop`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "ev", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, rename = "CaptureEvent")]
pub enum Event {
    State {
        state: WorkerState,
        elapsed_us: Us,
    },
    /// Seconds left before the start gate; held while the gate or a mic rule asks.
    Countdown {
        left_s: u32,
    },
    /// These sources aren't sending anything yet: "Record without it" (`WithoutSource`) or "Cancel".
    Gate {
        missing: Vec<SourceId>,
    },
    /// A source's peak over the last 100 ms, during the countdown (the mic meter).
    Meter {
        source: SourceId,
        peak_dbfs: f32,
    },
    Health {
        note: CodedError,
        level: Level,
        source: Option<SourceId>,
    },
    /// Once a second, whatever the state: the supervisor's sign of life.
    Heartbeat {
        n: u64,
        queued_ms: u32,
        free_bytes: Option<u64>,
    },
    /// Check camera's framing preview, at most 15 a second; the recorder's live view, about 10 in its small view and up
    /// to 30 in a big one.
    Preview {
        jpeg_base64: String,
        width: u32,
        height: u32,
    },
    /// The take is complete in its folder (`session` is its id); the app imports it, unless `StartOver` came first.
    Finished {
        session: String,
    },
    /// The person pressed Start over: the take's files are finished as for Stop, then the app moves its
    /// folder to the Trash (never imports it) and starts the next take with the same sources and settings.
    StartOver,
    Failed {
        error: CodedError,
    },
}

/// `ev` as one line: JSON and a newline.
pub fn encode(ev: &Event) -> Vec<u8> {
    // Can't fail: every map key is a string, and serde_json writes a non-finite number as null.
    let mut out = serde_json::to_vec(ev).expect("an event serialises");
    out.push(b'\n');
    out
}

/// One line of the app's: None (logged) when it isn't a command. Blank lines are None and say nothing.
pub fn decode_command(line: &str) -> Option<Command> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str(line)
        .inspect_err(|e| tracing::warn!(error = %e, "worker: a line from the app isn't a command"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_events_are_one_json_line_each() {
        assert_eq!(decode_command(r#"{"cmd":"stop"}"#), Some(Command::Stop));
        assert_eq!(
            decode_command(r#" {"cmd":"withoutSource","source":"mic"} "#),
            Some(Command::WithoutSource {
                source: "mic".into()
            })
        );
        assert_eq!(decode_command("stop"), None);
        assert_eq!(decode_command(""), None);
        let line = encode(&Event::Heartbeat {
            n: 3,
            queued_ms: 40,
            free_bytes: None,
        });
        assert_eq!(
            line,
            b"{\"ev\":\"heartbeat\",\"n\":3,\"queuedMs\":40,\"freeBytes\":null}\n"
        );
    }
}
