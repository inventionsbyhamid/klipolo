//! Screen recording, the app's side: it owns the take's worker and tells the page what happens to the take.
//!
//! - **The worker** is this binary as `klipolo-app --record-worker <take folder>`, in a session and process group of its
//!   own, so it outlives an editor crash (`launch`). The app watches its heartbeat, kills a hung one and resumes a
//!   crashed one as a new run, at most 3 times in 10 minutes (`supervise`, `events`, `ticker`).
//! - **The take** (`take`, `send`): arming launches the worker and captures nothing; only the person's Record press
//!   starts it. While it records, Klipolo's window hides (`window`); when it is saved, failed or over, the window shows
//!   again. A take whose screen's encoder gives up mid-take is stopped and saved; one whose camera's does records on
//!   without the camera (`outcomes`). When Klipolo exits, a take that hasn't started is disarmed (`active`).
//! - **The landing** (`sink`, `import`, `destination`, `zoom`): a saved take lands in the project open when it was armed
//!   (opened again should another be open by then), or a new "Recording, {Mon D}" for a take from the start screen, as
//!   one undo step, then its clicks' auto zoom as a second, and the page hears `recorder-imported`. The engine's
//!   background media work waits from arming until the take has landed.
//! - **Quitting during a take** (`quitting`, `app_quit`): the exit is held, the take stopped, and Klipolo exits once it
//!   has landed, or after 30 s.
//! - **Setup** (`setup`, `config`): the Record dialog's data from the backend without capture or prompts, and a take's
//!   config from the person's options and the app's fixed choices.
//! - **The permission steps** (`permissions`, `screen_reopen`): Allow screen recording, Reopen Klipolo and Open privacy
//!   settings, each on the person's own press.
//! - **A test run** (`KLIPOLO_TEST`): the recorder makes no backend and launches no worker; every command answers
//!   "Recording is off in this test run." (`state`).

mod active;
mod app_quit;
pub mod commands;
mod config;
mod destination;
mod env;
mod events;
mod import;
mod launch;
mod outcomes;
mod permissions;
mod quitting;
mod screen_reopen;
mod send;
mod setup;
mod sink;
mod state;
mod supervise;
mod take;
mod ticker;
pub mod types;
mod window;
mod words;
mod zoom;

#[cfg(test)]
mod test_launch;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod test_takes;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_end;
#[cfg(test)]
mod tests_import;
#[cfg(test)]
mod tests_permissions;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_stop;
#[cfg(test)]
mod tests_supervise;
#[cfg(test)]
mod tests_zoom;

pub use active::release_on_exit;
pub use app_quit::guard_quit;
pub use state::RecorderState;
