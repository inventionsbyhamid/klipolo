//! The recorder: `core`, the session as a state machine with no threads and no IO (with `start`, `source`,
//! `admit`, `runs`, `rules`, `heals`, `mic_swap`, `inputs`, `media_in`, `cursor_in`, `status` and `outcomes` holding
//! its parts); `resume`, going
//! on with a take after the worker crashed; `threads`, the core on threads with a backend, the writer and the shell
//! (`prepare`, `driver`, `stopping`, `feeds`, `pipes`, `controller`, `own_mic`); `finish`, Stop's last steps on the files; `check`,
//! Check camera's preview, and `live_view`, the recorder's live view of the chosen screen, both over the
//! previews' shared loop (`preview_loop`).

mod admit;
pub mod check;
mod controller;
pub mod core;
mod cursor_burn;
mod cursor_in;
mod driver;
mod effect;
mod feeds;
pub mod finish;
mod heals;
mod inputs;
pub mod live_view;
mod media_in;
mod mic_swap;
mod outcomes;
mod own_mic;
mod pipes;
mod prepare;
mod preview_loop;
pub mod resume;
mod rules;
mod runs;
#[cfg(feature = "synthetic")]
mod scripted;
mod source;
mod start;
mod status;
mod stopping;
pub mod threads;

pub use feeds::remove_empty_take;
