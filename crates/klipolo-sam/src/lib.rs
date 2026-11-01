//! SAM 3.1 video segmentation: parse the per-frame output into boxes.
//!
//! The model answers with one text line per frame:
//! `<Nf>ID<|box;x1=..;y1=..;x2=..;y2=..;w=..;h=..;c=..|><|mask;x=0;y=0;c=..;data=H,W,<enc>payload|>,ID...`.
//! Boxes come back inclusive in source pixels; [`BoxPx`] holds them half-open.

pub mod parse;

pub use parse::{parse_frames, BoxPx, FrameObjects, Obj, ParseError};
