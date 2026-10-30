//! How a compositor treats shaders that aren't built in.
//!
//! - **`Interactive`** is only the app's preview (the engine session `commands/engine.rs` builds). A `Checked` shader's
//!   pipeline compiles on a thread of its own (`effects::compile_thread`), so the drawing thread never waits for a
//!   driver's compile, and it is drawn only once the app said its crash-guard mark is on disk: until then the layer
//!   draws without the effect. Its passes are sampled and slowed down when over budget (`effects::sampling`).
//! - **`Contained`** is everything else: the export worker, the render worker, `Headless` and every test. A `Checked`
//!   shader compiles before the frame, synchronously, inside the device's locked error scopes, with no handshake: a crash
//!   or hang there is contained by the process itself (the render worker's deadline, the export's retry).
//!
//! The mode is a required argument of `Compositor::new` and of every `GpuBackend` constructor: there is no default,
//! and a backend keeps its mode, so a compositor it makes again (the warm-up's fallback, after a device loss) has it.

/// See the module's doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    /// The app's preview: asynchronous, guard-gated pipelines for checked shaders.
    Interactive,
    /// The export worker, the render worker and tests: checked shaders compile before the frame, no handshake.
    Contained,
}
