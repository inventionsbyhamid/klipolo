//! Media the chat and the library place on the timeline as ops: background music that fits the video ([`music`]),
//! sound effects at a moment ([`sound`]), pictures ([`image`]), and words: text presets and title templates
//! ([`text`]), and words that come in by letter, word or line ([`text_motion`]); and sounds and pictures let go on
//! the timeline, one after another ([`place`]).

pub mod image;
pub mod music;
pub mod place;
pub mod sound;
pub mod text;
pub mod text_motion;
