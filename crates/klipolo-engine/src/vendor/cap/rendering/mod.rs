//! Trimmed Cap `crates/rendering` (see NOTICE): decoders, YUV conversion, the composite pipeline, layers and the
//! frame pipeline. Klipolo's compositor (`crate::render`) drives them.

pub mod composite_frame;
pub mod decoder;
pub mod frame_pipeline;
mod gpu;
pub mod iosurface_texture;
pub mod layers;
pub mod spring_mass_damper;
pub mod text;
pub mod yuv_converter;
pub mod zoom;
pub mod zoom_spring;

// Cap's rendering/src/lib.rs:69 re-exports these; vendored files reach them as `crate::vendor::cap::rendering::*`.
pub use decoder::{DecodedFrame, PixelFormat};
pub use gpu::*;
