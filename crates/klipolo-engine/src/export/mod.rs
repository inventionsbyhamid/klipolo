//! Export: the parent plans a job folder (a manifest, the frozen project, then segments of about 30 s
//! per format) and runs the app binary as `--export-worker <job folder>/job.json`. The worker decodes once per frame,
//! draws every format on the GPU and feeds one VideoToolbox session per format, cutting closed-GOP segments that are
//! fsynced and verified before they count; it renders the sound in one pass and joins each format's segments into
//! its file by stream copy. A failed, crashed, quit or paused job continues from its finished segments; finished
//! files are renamed into place.

mod attempt;
pub mod audio;
mod boxes;
pub mod caption_file;
mod check;
pub mod codecs;
pub mod contrib;
pub(crate) mod deliver;
pub mod encode;
pub mod finalize;
pub mod forward;
pub mod job;
pub mod jobs;
pub(crate) mod launch;
pub mod naming;
mod notes;
mod pass;
mod passes;
pub(crate) mod plumbing;
pub mod prepare;
pub mod protocol;
pub mod quality;
mod registry;
pub mod runner;
pub mod segment;
pub mod shared;
pub mod sound;
mod sound_wait;
pub mod worker;
