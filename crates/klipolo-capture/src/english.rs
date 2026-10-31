//! klipolo-capture's codes and their English: every `capture.*` code the recorder sends. They are all declared
//! here up front, so the later tasks use them and invent none. `msg!` and `code!` refuse to compile a code
//! that isn't here. The UI words every code in the person's language (its descriptors come from
//! `apps/desktop/src/bindings/captureCodes.json`); logs and MCP replies read `english()`. The UI's own messages live under
//! `recorder.*`, `source.*` and `marker.*`, never under `capture`, which is Rust's; the permission steps' copy is coded
//! here too, beside the permissions' own notes.
//!
//! Parameters:
//! - `atUs` is a time in the recording and `durationUs` a recording's length, both µs, worded as clocks. `timeAtMs` is
//!   a moment (ms since 1970), worded in the person's time zone. `left` (the space left, `capture.menu.space`) is a
//!   nested `capture.duration.*` code from `hours_minutes`: "1 h 10 min".
//! - `minutes`, `seconds` and `fps` are numbers.
//! - `device`, `fallback` and `app` are names the OS gives (user text). `source` is a nested `capture.source.*` code,
//!   or the person's own name for the source. `why` (`capture.note.didntStart`) is the nested `capture.error.*` code of
//!   what stopped the source.
//! - `place` is a nested `capture.place.*` code from `CaptureCaps.indicator`; `os` and `version` are
//!   text; `keys` is the shortcut as the shell shows it; `time` (`capture.hud.recording`) is the take's length as the
//!   shell's clock shows it ("12:34").
//! - `samples` and `channels` (`capture.note.soundBlock`: the dropped block's shape) are for the log only.
//! - `status` (`capture.error.encoder`), `detail` (`capture.error.os`) and `atUs` in `capture.recovered.partKept`
//!   (where the part left out starts) are for the log only; the messages don't
//!   show them. So are `reason` in `capture.health.lost` ("stopped", "stalled", "encoder" with its `status`, "missing",
//!   "blank" or "withVideo") and `dropped` and `frames` in `capture.health.dropping` (frames dropped, and delivered or due, over
//!   the last 10 s).

use klipolo_model::coded::{CodedError, Messages};

macro_rules! msg {
    ($($t:tt)*) => { klipolo_model::coded_msg!($crate::english::ENGLISH, $($t)*) };
}
/// The code itself, checked like `msg!`.
#[allow(unused_macros)]
macro_rules! code {
    ($c:literal) => {
        klipolo_model::coded_code!($crate::english::ENGLISH, $c)
    };
}
#[allow(unused_imports)]
pub(crate) use {code, msg};

/// A length of `minutes` as the nested code a message's `left` takes: "1 h 10 min", "2 h", "45 min".
pub fn hours_minutes(minutes: u64) -> CodedError {
    match (minutes / 60, minutes % 60) {
        (0, m) => msg!("capture.duration.minutes", minutes = m as usize),
        (h, 0) => msg!("capture.duration.hours", hours = h as usize),
        (h, m) => msg!(
            "capture.duration.hoursMinutes",
            hours = h as usize,
            minutes = m as usize
        ),
    }
}

/// `e` in English, from this crate's table (nested `capture.*` codes included).
pub fn english(e: &CodedError) -> String {
    klipolo_model::coded::english(&[ENGLISH], e)
}

#[rustfmt::skip]
pub const ENGLISH: Messages = &[
    // Names Klipolo makes up for sources: a source's label is the code until the person renames it.
    ("capture.source.screen", "Screen"),
    ("capture.source.game", "Game"),
    ("capture.source.camera", "Camera"),
    ("capture.source.mic", "Mic"),
    ("capture.source.gameSound", "Game sound"),
    ("capture.source.computerSound", "Computer sound"),
    // An app's or a window's own sound ("Record the app's sound").
    ("capture.source.appSound", "App sound"),
    // Health while recording.
    ("capture.health.micZeros", "Your microphone is sending no sound. It may be muted or blocked."),
    ("capture.health.micQuiet", "We can barely hear anything from {device}."),
    ("capture.health.micNotHeard", "Haven't heard you yet. Is {device} the microphone you meant?"),
    ("capture.health.diskLow", "About {minutes} min of space left."),
    ("capture.health.slowDrive", "This drive can't keep up."),
    ("capture.health.dropping", "Some frames are being dropped. Try the Light quality."),
    ("capture.health.lost", "{source} disconnected."),
    ("capture.health.reconnecting", "{source} is reconnecting…"),
    ("capture.health.restarting", "Restarting screen capture…"),
    ("capture.health.micFallback", "{device} disconnected. Recording with {fallback} instead."),
    ("capture.health.micBack", "{device} is back. Recording with it again."),
    ("capture.health.hot", "Your computer is very hot. Recording may drop frames."),
    ("capture.health.appQuit", "{app} quit. Recording is paused until it comes back."),
    ("capture.health.presenterOverlay", "Presenter Overlay is on, so your camera is in the screen video too."),
    ("capture.health.readBack", "Something is wrong with what was saved so far. Check your disk, or stop and try again."),
    // Why a take stopped by itself; it is always saved.
    ("capture.stop.disk", "Your disk is almost full, so the recording stopped and was saved."),
    ("capture.stop.slowDrive", "This drive couldn't keep up, so the recording stopped and was saved."),
    ("capture.stop.sharing", "Screen sharing was stopped, so the recording stopped and was saved."),
    ("capture.stop.permission", "Klipolo lost permission to record, so the recording stopped and was saved."),
    ("capture.stop.noSources", "Every source stopped, so the recording was saved."),
    ("capture.stop.crashLoop", "The recorder kept stopping, so the recording was saved."),
    ("capture.stop.slept", "Your computer slept for a long time, so the recording stopped and was saved."),
    // The app stops a take once the worker gives a source up because its video encoder kept failing.
    ("capture.stop.encoder", "The video encoder stopped, so the recording stopped and was saved."),
    // "Stop sharing" (the system's screen-sharing control) before the start gate: a start refused, so nothing was
    // recorded or saved.
    ("capture.stop.sharingBeforeStart", "Screen sharing was turned off before the recording started, so nothing was recorded."),
    // Start over: the last take couldn't be moved aside at all (it waits on Home), or Undo couldn't put it
    // back (it was already let go to the Trash, say).
    ("capture.startOver.kept", "Started over. The last take couldn't be moved to the Trash, so it waits on Home."),
    ("capture.startOver.undoFailed", "The last take couldn't be put back. If it isn't on Home, look for it in the Trash."),
    // The start gate, pauses and notes after a take.
    ("capture.gate.waiting", "{source} isn't sending anything yet."),
    ("capture.paused.sleep", "Recording paused while your computer slept."),
    ("capture.paused.lock", "Recording paused while your screen was locked."),
    ("capture.paused.focus", "Paused while {app} isn't in front."),
    ("capture.note.gap", "{source} stopped for {seconds} s at {atUs}. Everything else was recorded."),
    // Milder notes from the recorder: a dropped block of sound, a sound source that stayed silent.
    ("capture.note.soundBlock", "A moment of sound was dropped."),
    ("capture.note.silentSound", "{source} was silent the whole recording."),
    ("capture.note.micFlapping", "{source} keeps disconnecting, so we'll wait until it settles before using it again."),
    ("capture.note.micDropping", "{source} keeps disconnecting. We record it again each time it's back."),
    ("capture.note.levels", "{source}'s sound level couldn't be measured, so it plays as it was recorded."),
    ("capture.note.encoderFallback", "{source} was recorded with another video encoder, as the usual one didn't start."),
    // A source that couldn't start, named, with why: the countdown's and the chat's note.
    ("capture.note.didntStart", "{source} didn't start. {why}"),
    ("capture.note.cursorBurned", "From {atUs} the cursor is part of the picture: Klipolo couldn't follow it there."),
    ("capture.note.cursorSampled", "From {atUs} clicks were timed less exactly, as the system stopped sending them."),
    ("capture.note.cursorShapes", "The cursor changed shape too many times, so some of its later shapes weren't recorded."),
    // Preflight: checked before Record, without capture.
    ("capture.preflight.diskBlock", "There isn't enough space to record. Free up at least 2 GB."),
    ("capture.preflight.diskShort", "Less than 30 minutes of space left at this quality. Free up space or choose Light."),
    ("capture.preflight.virtualMic", "{device} is a virtual microphone. Your voice may not be recorded."),
    ("capture.preflight.bluetoothMic", "Your headphones' microphone lowers the sound quality while recording, so we picked {fallback}."),
    ("capture.preflight.micMissing", "The microphone you chose isn't connected, so we picked {fallback}."),
    ("capture.preflight.cameraBusy", "{device} is in use by another app."),
    ("capture.preflight.cameraOff", "{device} is turned off or covered."),
    ("capture.preflight.cameraWithScreen", "This computer can't record your camera at the same time as the screen."),
    ("capture.preflight.lowBattery", "Your battery is low. Plug in for long recordings."),
    ("capture.preflight.slowDrive", "This drive is slow. Long recordings may stop early."),
    ("capture.preflight.noEncoder", "This computer has no video encoder Klipolo can record with."),
    ("capture.preflight.osTooOld", "Recording needs {os} {version} or later."),
    ("capture.preflight.profilePreset", "The quality you chose for {app} isn't available on this computer now, so we picked the best one for it."),
    ("capture.preflight.cursorBurned", "This computer keeps the cursor in the picture, so it can't be restyled later."),
    ("capture.preflight.networkFolder", "Recordings can't be saved safely in this folder. Choose a folder on this computer, not on a network drive."),
    // macOS applies a Screen Recording grant only once Klipolo reopens, so never "try again".
    ("capture.permission.screen", "Allow Klipolo to record your screen, then reopen Klipolo."),
    // The recorder's Screen Recording step, before and after its button's press, and its buttons; and the
    // button beside a camera's or a microphone's refused permission.
    ("capture.permission.screenNeeded", "Klipolo needs your permission to record your screen."),
    ("capture.permission.screenTurnOn", "Turn on Klipolo in the list that opened, then reopen Klipolo."),
    // After Reopen Klipolo, while macOS still says no: why, and how to fix it, beside Open privacy
    // settings. Klipolo never resets or changes the setting itself.
    ("capture.permission.screenStillRefused", "macOS still isn't letting Klipolo record the screen. This can happen when an older copy of Klipolo is in the list."),
    ("capture.permission.screenRemoveOld", "Open Screen & System Audio Recording, select Klipolo, remove it with −, then press Allow again."),
    ("capture.permission.allowScreen", "Allow screen recording"),
    ("capture.permission.reopen", "Reopen Klipolo"),
    ("capture.permission.openSettings", "Open privacy settings"),
    ("capture.permission.camera", "Allow Klipolo to use your camera in your computer's privacy settings, then try again."),
    ("capture.permission.mic", "Allow Klipolo to use your microphone in your computer's privacy settings, then try again."),
    // The recorder's live view: its one quiet line when it can't run (any failure but Screen Recording's,
    // whose step takes its place, or a computer that has none, which shows nothing).
    ("capture.liveView.unavailable", "The live view isn't available right now."),
    // Unfinished takes on Home.
    ("capture.recovered.interrupted", "Klipolo closed while recording at {timeAtMs}. {durationUs} was saved."),
    ("capture.recovered.ready", "Your recording from {timeAtMs} is ready."),
    ("capture.recovered.live", "A recording is still running. Stop it from {place}."),
    ("capture.recovered.approximate", "Some timing in this recording is approximate."),
    ("capture.recovered.partKept", "Part of {source} couldn't be repaired automatically. Its files are kept in the take's folder."),
    // A whole source left out (every run of it), so not "Part of".
    ("capture.recovered.sourceKept", "{source} couldn't be repaired automatically. Its files are kept in the take's folder."),
    // Home's Open on a take that couldn't be repaired now (a disk error), or isn't there; and Delete… that couldn't
    // move it to the Trash. Nothing is lost either way.
    ("capture.recovered.failed", "This recording couldn't be opened. Its files are kept in the take's folder."),
    // Opening it needs one line written to its folder first: a full or read-only drive is
    // named, so the person knows what to do. The take stays on Home.
    ("capture.recovered.driveFull", "This recording couldn't be opened because its drive is full. Free up some space, then open it again."),
    ("capture.recovered.driveReadOnly", "This recording couldn't be opened because its drive is read-only. Reconnect the drive or make it writable, then open it again."),
    ("capture.recovered.trashFailed", "This recording couldn't be moved to the Trash. Its files are kept in the take's folder."),
    // The same on Windows, whose Trash is the Recycle Bin.
    ("capture.recovered.recycleFailed", "This recording couldn't be moved to the Recycle Bin. Its files are kept in the take's folder."),
    // Windows' Recycle Bin can't take it (too big, or the drive has none): never deleted outright.
    ("capture.recovered.trashTooBig", "This recording is too big for the Recycle Bin, so it's still in its folder."),
    ("capture.busy", "A recording is already running. Stop it from {place}."),
    // Arming while v1 dictation listens to the mic: the mic is the dictation's until it ends.
    ("capture.busyDictating", "Finish dictating first, then record."),
    ("capture.orphan", "Klipolo closed. Still recording."),
    ("capture.quit.ask", "A recording is running. Stop and save it, then quit?"),
    ("capture.import.noVideo", "This take has no picture to edit. Its sound files are in the recording's folder."),
    ("capture.import.videoUnreadable", "This take's picture couldn't be read, so there's nothing to edit. Its files are kept in the take's folder."),
    ("capture.import.unreadable", "Part of {source} ({seconds} s at {atUs}) couldn't be read, so it isn't in the edit. Its file is kept in the take's folder."),
    // The recorder's menu-bar, notification-area or tray item.
    ("capture.menu.stop", "Stop and save"),
    ("capture.menu.pause", "Pause"),
    ("capture.menu.resume", "Resume"),
    ("capture.menu.marker", "Mark this moment"),
    ("capture.menu.space", "Space left: about {left}"),
    ("capture.menu.fps", "{source}: {fps} fps"),
    ("capture.menu.reconnecting", "{source}: reconnecting…"),
    ("capture.menu.idle", "{source}"),
    ("capture.menu.ready", "Ready: press {keys} in your game"),
    ("capture.menu.cancel", "Cancel"),
    ("capture.menu.startOver", "Start over"),
    // A shortcut the shell couldn't register (`shell_view::KeysController::problem`).
    ("capture.keys.taken", "{keys} is used by another app, so it won't work in this recording."),
    // The HUD's pill while recording (`shell_view::HudView`).
    ("capture.hud.recording", "● REC {time}"),
    // After the held countdown's reason, in a take the start key began (its editor is hidden).
    ("capture.hud.answer", "Press {keys} to record without it, or Cancel from {place}."),
    // A length in hours and minutes (`hours_minutes`), nested in the messages above.
    ("capture.duration.hoursMinutes", "{hours} h {minutes} min"),
    ("capture.duration.hours", "{hours} h"),
    ("capture.duration.minutes", "{minutes} min"),
    // Where the recorder's item lives: the `place` of the messages above.
    ("capture.place.menuBar", "the menu bar"),
    ("capture.place.notificationArea", "the notification area"),
    ("capture.place.tray", "the system tray"),
    // A pipe that couldn't start (`PipeError::coded`).
    ("capture.error.unsupported", "This computer can't record that."),
    ("capture.error.permission", "Klipolo isn't allowed to record that yet."),
    ("capture.error.permissionPending", "Your computer is still asking whether Klipolo may record this. Answer that question, then try again."),
    ("capture.error.noDevice", "That device isn't connected."),
    ("capture.error.busy", "That device is in use."),
    ("capture.error.encoder", "The video encoder stopped."),
    ("capture.error.os", "Recording failed."),
    // A command or a close for a take (or Check camera's preview) that has already ended: nothing is done.
    ("capture.error.staleTake", "That recording has already ended."),
    ("capture.error.driveStuck", "The drive stopped answering, so the recording couldn't be finished. What reached it will be recovered when Klipolo opens."),
    // A test run (`KLIPOLO_TEST`): the app's recorder makes no backend and launches no worker.
    ("capture.error.testRun", "Recording is off in this test run."),
];
