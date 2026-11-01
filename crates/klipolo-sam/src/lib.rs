//! SAM 3.1 video segmentation: the request for a noun phrase (`request`), and the per-frame output read into boxes
//! (`parse`), strictly or skipping lines that don't read.
//!
//! The model answers with one text line per frame:
//! `<Nf>ID<|box;x1=..;y1=..;x2=..;y2=..;w=..;h=..;c=..|><|mask;x=0;y=0;c=..;data=H,W,<enc>payload|>,ID...`.
//! Boxes come back inclusive in source pixels; [`BoxPx`] holds them half-open.

pub mod parse;
pub mod request;

pub use parse::{parse_frames, parse_frames_lenient, BoxPx, FrameObjects, Obj, ParseError};
pub use request::{concept_request, MODEL};
