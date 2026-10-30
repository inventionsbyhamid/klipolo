//! Checked shaders' pipelines compiled off the drawing thread (`RenderMode::Interactive` only). A driver may
//! take hundreds of milliseconds to compile an agent's shader (Metal: 28–500 ms on the Intel Mac); the preview never
//! waits for it. The drawing thread asks (`request`) and polls at each frame's start (`take_ready`), never blocking;
//! meanwhile the layer draws without the effect. The thread compiles exactly as `pipelines::compile` does on the drawing
//! thread, inside the device's locked error scopes (`scopes::ScopeLock`), so its errors and the drawing thread's
//! never mix. A pipeline it hands over still waits for the crash guard's mark (`handshake`) before it draws.
//!
//! One thread per stage, made with the first request (a project without agent-made effects starts none), ending when
//! the stage goes (it finishes the compile it is in). Bounded: one request per shader key at a time (the stage asks once).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use tokio::sync::Notify;

use crate::registry::ShaderEffect;
use crate::render::device::RenderDevice;

/// What the thread hands back: the shader's key and its pipeline, or why it couldn't be made.
pub type Compiled = (u64, Result<wgpu::RenderPipeline, String>);

pub struct PipelineCompiler {
    requests: mpsc::Sender<Arc<ShaderEffect>>,
    ready: Arc<Mutex<Vec<Compiled>>>,
    /// Asked for and not handed back yet.
    pending: Arc<AtomicUsize>,
    wake: Arc<Mutex<Option<Arc<Notify>>>>,
}

impl PipelineCompiler {
    /// Starts the thread on `device`, compiling with `layout` (the effect stage's).
    pub fn new(device: Arc<RenderDevice>, layout: wgpu::PipelineLayout) -> PipelineCompiler {
        let (requests, rx) = mpsc::channel::<Arc<ShaderEffect>>();
        let ready = Arc::new(Mutex::new(Vec::new()));
        let pending = Arc::new(AtomicUsize::new(0));
        let wake: Arc<Mutex<Option<Arc<Notify>>>> = Arc::default();
        let (out, left, woken) = (ready.clone(), pending.clone(), wake.clone());
        let spawned = std::thread::Builder::new()
            .name("klipolo-pipelines".into())
            .spawn(move || {
                // Someone is waiting for the effect to appear, but not on this thread's every frame.
                crate::qos::set_current(crate::qos::Qos::UserInitiated);
                while let Ok(shader) = rx.recv() {
                    // A parser or driver that panics fails only this shader: its scopes are popped
                    // as it unwinds (`ScopeLock::scoped`), the thread goes on, and its mark closes when it is handed back.
                    let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        super::pipelines::compile(&device, &layout, &shader)
                    }))
                    .unwrap_or_else(|_| Err("the shader compiler stopped on it".into()));
                    if let Err(error) = &made {
                        tracing::warn!(effect = %shader.id, %error, "effect shader");
                    }
                    super::made();
                    out.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((shader.key, made));
                    left.fetch_sub(1, Ordering::SeqCst);
                    if let Some(wake) = &*woken.lock().unwrap_or_else(PoisonError::into_inner) {
                        wake.notify_one();
                    }
                }
            });
        if let Err(error) = spawned {
            // The receiver went with the closure: requests fail, and those effects stay out of the preview.
            tracing::error!(%error, "the pipeline compile thread");
        }
        PipelineCompiler {
            requests,
            ready,
            pending,
            wake,
        }
    }

    /// Fired once each pipeline is handed back (the preview draws its still again).
    pub fn set_wake(&self, wake: Arc<Notify>) {
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(wake);
    }

    /// Compiles `shader`'s pipeline on the thread; it comes back from `take_ready`.
    pub fn request(&self, shader: Arc<ShaderEffect>) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        if self.requests.send(shader).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// The pipelines made since the last call (never waits).
    pub fn take_ready(&self) -> Vec<Compiled> {
        std::mem::take(&mut *self.ready.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Requests not handed back yet (tests wait for 0).
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }
}
