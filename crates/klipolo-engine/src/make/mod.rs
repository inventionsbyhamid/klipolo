//! The sandboxed render worker that checks, measures and previews agent-made effects and scenes: its JSON-lines
//! Its JSON-lines protocol of bytes (`protocol`), its session folder (`session`), its own sandbox
//! (`sandbox`), its loop (`worker`) and what a measurement found (`budget`).
//! The work it does: the draft layer it draws (`draft`), the measurement (`measure`, stepped first by
//! `steps`, at the settings `envelope` plans), its frames and their statistics (`frames`, `stats`), the output
//! check's audit (`audit`), its GPU and fonts (`gpu`, `fonts`) and its sizes (`sizes`); SVG flattened into
//! scene paths (`svg`); a scene's costly settings (`scene_check`).

/// A line on the worker's log (stderr) that never panics: once the app is gone, the pipe it reads is closed and
/// `eprintln!` would panic.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}
pub(crate) use say;

pub mod audit;
pub mod budget;
pub mod draft;
pub mod envelope;
pub mod fonts;
pub mod frames;
pub mod gpu;
pub mod measure;
pub mod protocol;
pub mod sandbox;
pub mod scene_check;
pub mod session;
pub mod sizes;
pub mod stats;
pub mod steps;
pub mod svg;
pub mod worker;
