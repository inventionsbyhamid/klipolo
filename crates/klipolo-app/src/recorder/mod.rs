//! Screen recording, the app's side: it owns the take's worker and tells the page what happens to the take.
//!
//! - **The worker** is this binary as `klipolo-app --record-worker <take folder>`, in a session and process group of its
//!   own, so it outlives an editor crash (`launch`). The app watches its heartbeat, kills a hung one and resumes a
//!   crashed one as a new run, at most 3 times in 10 minutes (`supervise`, `events`, `ticker`).
//! - **The take** (`take`, `send`): arming launches the worker and captures nothing; only the person's Record press
//!   starts it. While it records, Klipolo's window hides (`window`); when it is saved, failed or over, the window shows
//!   again. A take whose screen's encoder gives up mid-take is stopped and saved; one whose camera's does records on
//!   without the camera (`outcomes`). When Klipolo exits, a take that hasn't started is disarmed (`active`).
//! - **Setup** (`setup`, `config`): the Record dialog's data from the backend without capture or prompts, and a take's
//!   config from the person's options and the app's fixed choices.
//! - **The permission steps** (`permissions`, `screen_reopen`): Allow screen recording, Reopen Klipolo and Open privacy
//!   settings, each on the person's own press.
//! - **A test run** (`KLIPOLO_TEST`): the recorder makes no backend and launches no worker; every command answers
//!   "Recording is off in this test run." (`state`).

mod active;
pub mod commands;
mod config;
mod env;
mod events;
mod launch;
mod outcomes;
mod permissions;
mod screen_reopen;
mod send;
mod setup;
mod state;
mod supervise;
mod take;
mod ticker;
pub mod types;
mod window;
mod words;

#[cfg(test)]
mod test_launch;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_permissions;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_stop;
#[cfg(test)]
mod tests_supervise;

#[allow(
    unused_imports,
    reason = "read by the quit guard and the engine's hold, which come with the take's landing"
)]
pub use active::active;
pub use active::release_on_exit;
pub use state::RecorderState;
