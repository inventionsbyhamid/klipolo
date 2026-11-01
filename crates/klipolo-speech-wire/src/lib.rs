//! Klipolo's speech timing rules, pure Rust:
//! - `words`: a word as a recognizer heard it, `snap_to_speech`, which moves its edges past pauses onto its sound, and
//!   `refine_edges`, which moves edges that sit on a recognizer's 80 ms encoder steps onto the sound's own edges;
//! - `timing`: how close a transcript is to its reference (word starts against labels, the word error rate), and each
//!   language's hesitations (`fillers`).

pub mod timing;
pub mod words;
