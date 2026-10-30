//! The caption renderer. A page is shaped once (`layout`: each word in the
//! style's face at its size step and keyword scale, the lines balanced and the page autofitted), and each word is drawn
//! once into its own cell of the page's atlas as four packed masks (`atlas`). The drawer (`CaptionDrawer`) keeps a few
//! pages' layouts and atlases per format, built on a thread of their own (`worker`), and draws a frame's words from them
//! as instanced quads (`draw`), coloured and moved by the frame's state (`plan::captions::frame`): nothing is laid out
//! or rasterised per frame, nor on the render thread.

pub mod atlas;
pub mod blur;
mod builds;
mod cache;
mod cells;
mod draw;
mod draw_pipe;
mod drawer;
pub mod layout;
mod measure;
mod pipes;
mod shaping;
mod testing;
mod worker;

#[doc(hidden)]
pub use crate::render::build_times::BuildTime;
#[doc(hidden)]
pub use atlas::forbid_builds_on_this_thread;
#[doc(hidden)]
pub use cells::blur_sigmas;
pub use drawer::{CaptionDrawer, CaptionStats, FORMAT_BYTES, MAX_IN_FLIGHT, PER_FORMAT};
#[doc(hidden)]
pub use testing::{TestAtlas, test_atlas, test_atlas_with, test_font_system};
#[doc(hidden)]
pub use worker::{build_times, time_builds};
