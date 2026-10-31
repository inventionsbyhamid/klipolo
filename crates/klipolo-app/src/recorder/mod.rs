//! Screen recording, the app's side: it owns the take's worker and tells the page what happens to the take.
//!
//! - **The worker** is this binary as `klipolo-app --record-worker <take folder>`, in a session and process group of its
//!   own, so it outlives an editor crash (`launch`). The app watches its heartbeat, kills a hung one and resumes a
//!   crashed one as a new run, at most 3 times in 10 minutes (`supervise`, `events`, `ticker`).
//! - **The take** (`take`, `send`): arming launches the worker and captures nothing; only the person's Record press
//!   starts it. While it records, Klipolo's window hides (`window`); when it is saved, failed or over, the window shows
//!   again. A take whose encoder gives up mid-take is stopped and saved (`outcomes`).
//! - **Setup** (`setup`, `config`): the Record dialog's data from the backend without capture or prompts, and a take's
//!   config from the person's options and the app's fixed choices.
//! - **The permission steps** (`permissions`, `screen_reopen`): Allow screen recording, Reopen Klipolo and Open privacy
//!   settings, each on the person's own press.
//! - **A test run** (`KLIPOLO_TEST`): the recorder makes no backend and launches no worker; every command answers
//!   "Recording is off in this test run." (`state`).

mod config;
mod env;
mod events;
mod launch;
mod outcomes;
mod send;
mod setup;
mod state;
mod supervise;
mod take;
mod ticker;
pub mod types;
mod words;

#[cfg(test)]
mod test_launch;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_stop;
#[cfg(test)]
mod tests_supervise;

pub use state::RecorderState;
