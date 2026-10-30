//! The device-wide error-scope lock. In wgpu 25 a device has one stack of error scopes, shared by
//! every thread: an error raised on one thread lands in whatever scope is on top, whichever thread pushed it. Every
//! `push_error_scope` … `pop_error_scope` pair in the engine therefore runs through [`ScopeLock::scoped`], holding the
//! device's lock for the whole pair: the compile thread's (`compile_thread`) and the drawing thread's first-use compiles,
//! and the render worker's audit draws. So one thread's error never lands in another's scope.
//!
//! What the lock can't cover: work outside any scope that raises an error while another thread's scope is open (a
//! drawing thread's validation error during a compile on the compile thread) lands in that scope, not in the device's
//! fault handler. Scopes are held only around pipeline and module creation, which raise their own errors at once.

use std::sync::{Mutex, PoisonError};

/// One per device (`RenderDevice::scopes`).
#[derive(Debug, Default)]
pub struct ScopeLock {
    lock: Mutex<()>,
}

impl ScopeLock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `f` inside one error scope per filter of `filters` (pushed in order, popped in reverse), with the device's
    /// lock held from the first push to the last pop: `f`'s result and the first error any scope caught (the innermost
    /// first). wgpu-core reports creation errors as they happen, so the scopes' futures are ready at once. A panic in
    /// `f` still pops every scope it pushed, so none is left to swallow the device's later errors.
    pub fn scoped<T>(
        &self,
        device: &wgpu::Device,
        filters: &[wgpu::ErrorFilter],
        f: impl FnOnce() -> T,
    ) -> (T, Option<wgpu::Error>) {
        let _held = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        for filter in filters {
            device.push_error_scope(*filter);
        }
        let mut pushed = Pushed {
            device,
            left: filters.len(),
            first: None,
        };
        let out = f();
        pushed.pop_all();
        (out, pushed.first.take())
    }
}

/// The scopes a `scoped` call pushed and hasn't popped yet: popped on drop too (a panic in its closure), while the lock
/// is still held (it was taken first, so it goes last).
struct Pushed<'a> {
    device: &'a wgpu::Device,
    left: usize,
    first: Option<wgpu::Error>,
}

impl Pushed<'_> {
    fn pop_all(&mut self) {
        while self.left > 0 {
            self.left -= 1;
            let caught = futures::executor::block_on(self.device.pop_error_scope());
            self.first = self.first.take().or(caught);
        }
    }
}

impl Drop for Pushed<'_> {
    fn drop(&mut self) {
        self.pop_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::ScopeLock;

    /// Two threads pushing and popping scopes on one device never see each other's error. Thread A opens its
    /// scope, tells B, and waits a while for B to open its own before it raises a validation error. Without the lock B
    /// opens its scope on top of A's, A's error lands in B's scope, and A's comes back empty; with the lock B opens its
    /// scope only once A's pair is done.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_threads_never_see_each_others_scope_errors() {
        let Some(dev) = crate::test_support::adapter_or_skip().await else {
            return;
        };
        let scopes = Arc::new(ScopeLock::new());
        let (opened_tx, opened_rx) = mpsc::channel::<()>();
        let (b_opened_tx, b_opened_rx) = mpsc::channel::<()>();
        let (a, b) = (dev.clone(), dev.clone());
        let (sa, sb) = (scopes.clone(), scopes.clone());
        let thread_a = std::thread::spawn(move || {
            sa.scoped(&a.device, &[wgpu::ErrorFilter::Validation], || {
                opened_tx.send(()).unwrap();
                // B may open its scope now, if nothing stops it.
                let _ = b_opened_rx.recv_timeout(Duration::from_millis(300));
                // A shader module that isn't WGSL: a validation error, now.
                drop(a.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("not WGSL"),
                    source: wgpu::ShaderSource::Wgsl("this isn't a shader".into()),
                }));
            })
            .1
        });
        opened_rx.recv().unwrap();
        let thread_b = std::thread::spawn(move || {
            sb.scoped(&b.device, &[wgpu::ErrorFilter::Validation], || {
                let _ = b_opened_tx.send(());
                // A valid buffer: no error of its own.
                drop(b.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("fine"),
                    size: 16,
                    usage: wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
                std::thread::sleep(Duration::from_millis(100));
            })
            .1
        });
        let (in_a, in_b) = (thread_a.join().unwrap(), thread_b.join().unwrap());
        assert!(in_a.is_some(), "A's own error is A's");
        assert!(in_b.is_none(), "B saw {in_b:?}");
    }

    /// A panic inside a scope pair (a parser that panics) still pops its scopes: an error raised
    /// outside any scope afterwards reaches the device's fault handler, and the lock is free.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_panic_inside_a_scope_pair_still_pops_its_scopes() {
        let Some(dev) = crate::test_support::adapter_or_skip().await else {
            return;
        };
        let filters = [wgpu::ErrorFilter::Internal, wgpu::ErrorFilter::Validation];
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dev.scopes
                .scoped(&dev.device, &filters, || panic!("a parser that panics"))
        }));
        assert!(panicked.is_err());
        let _ = dev.take_fault();
        drop(
            dev.device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("not WGSL"),
                    source: wgpu::ShaderSource::Wgsl("this isn't a shader".into()),
                }),
        );
        assert!(dev.take_fault().is_some(), "no scope was left pushed");
        let (_, caught) = dev.scopes.scoped(&dev.device, &filters, || ());
        assert!(caught.is_none());
    }
}
