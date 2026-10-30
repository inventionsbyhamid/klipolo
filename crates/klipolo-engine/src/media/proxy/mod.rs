//! Editing copies (proxies): short-GOP H.264 copies of sources that
//! scrub slowly (4K on Intel, long GOPs, 10-bit, HDR, no hardware decode, variable rate), used by the preview only;
//! export always reads the originals.
//! - `facts` reads what a source is; `decide` (pure) says whether it needs a copy and how big;
//! - `job` is the copy's folder and manifest (10 s segments), `record` what a finished copy was made from,
//!   `run` the segment loop with its pauses (user, heat, disk, busy encoder);
//! - `worker` is the out-of-process side (`--proxy-worker`), `protocol` its lines, `runner` the app's side.
//! - `numbering`: what numbering frames from the stream's start means for copies and jobs made before it.

pub mod decide;
pub mod facts;
pub mod job;
mod numbering;
pub mod protocol;
pub mod record;
pub mod run;
pub mod runner;
pub mod worker;
