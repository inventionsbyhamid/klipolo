//! Export: plan the files with a size and time estimate, start a batch on the one export slot, stop it,
//! and say what it wrote. The worker runs out of process (this app's own binary, `--export-worker`); the engine runs
//! it, segment by segment, and renames each finished file into place, never over an existing file.
//!
//! - **One file per format**, `<project name> 16x9.mp4`, in the folder the dialog shows (`Movies / Klipolo` unless the
//!   person picked another), `<name> 16x9 2.mp4` when that name is taken (`naming`).
//! - **What it draws is what the preview draws**: the open project as saved, the captions frozen from the finished
//!   transcripts the preview has (`captions`), and the fonts checked first (`fonts`).
//! - **Nothing half made is left behind**: Cancel deletes the job and its parts, and so does a run that fails, as
//!   Klipolo doesn't resume one (`run`). Quit while one runs asks first and stops it (the app's `commands::export`).
//!
//! No Tauri types here: the commands in `commands/export.rs` are thin layers over it, and `tests/export_mac.rs` drives
//! it with the real worker.

mod captions;
mod estimate;
mod fonts;
mod naming;

pub use captions::export_speech;
pub use estimate::{Estimate, apple_silicon, estimate};
pub use fonts::{missing_fonts, refuse_missing};
pub use naming::{format_word, plan_files};
