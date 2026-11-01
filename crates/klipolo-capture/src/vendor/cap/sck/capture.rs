// SPDX-License-Identifier: MIT
// MIT (Cap's `scap-screencapturekit`), Copyright (c) 2023-present Cap Software, Inc.
// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/scap-screencapturekit/src/capture.rs
// Klipolo changes:
// - a screen buffer without an image is delivered too (Cap dropped it): idle, blank and suspended frames carry only
//   their status, which the pipe counts (a still screen's re-feed is the pipe's encoder thread's own);
// - the delegate also reports `streamDidBecomeActive:` and `streamDidBecomeInactive:` (macOS 15.2; the target went
//   away and came back), through `with_active_cb`;
// - the class is registered as `KLIPOLO_SCK_CAPTURER` (Cap's `CAPTURER` could meet another copy in one process);
// - the sample handler queue is serial at user-interactive QoS (capture threads run at it), with
//   an autorelease pool per item as Cap's;
// - the audio output is added whenever the config captures audio, with no macOS 13 check (Klipolo's floor is 15);
// - `start` and `stop` are synchronous with a time limit (`start_within`, `stop_within`): the pipe runs on its own
//   thread with no async runtime, and a stream that never answers must not hold it; `nudger` asks the stream to apply
//   its settings again and reports its answer, with the error's code if there was one (a live stream answers without
//   one: a still screen told from a stuck stream);
// - no microphone output is delivered (`Frame::Mic` is gone: Klipolo records the mic with its own pipe); Cap's
//   `Frame::sample_buf`, `Frame::output_type`, `Frame::sample_buf_mut`, `VideoFrame::sample_buf_mut`/`image_buf` and
//   `AudioFrame::sample_buf_mut` were unused and are gone; the `workspace-hack` dependency of Cap's crate too.
// Klipolo changes:
// - `update_within` applies new settings (`updateConfiguration:`) and waits, with a time limit, for the stream's answer:
//   the cursor's live switch into the frames.

use std::sync::mpsc::sync_channel;
use std::time::Duration;

use cidre::{
    arc, cm, define_obj_type, dispatch, ns, objc,
    sc::{self, StreamDelegate, StreamDelegateImpl, StreamOutput, StreamOutputImpl},
};

define_obj_type!(
    pub CapturerCallbacks + StreamOutputImpl + StreamDelegateImpl + CapturerCallbacksDeallocation,
    CapturerCallbacksInner,
    KLIPOLO_SCK_CAPTURER
);

trait CapturerCallbacksDeallocation {
    fn cls_add_methods(cls: &objc::Class<objc::Id>) {
        extern "C" fn dealloc(callbacks: &mut CapturerCallbacks, selector: &objc::Sel) {
            let object = callbacks as *mut CapturerCallbacks;
            unsafe {
                let superclass_dealloc: unsafe extern "C" fn(*mut CapturerCallbacks, &objc::Sel) =
                    std::mem::transmute(objc::NS_OBJECT.method_impl(selector));
                std::ptr::drop_in_place(callbacks.inner_mut());
                superclass_dealloc(object, selector);
            }
        }

        // The pinned cidre macro drops only the Rust payload. Its later
        // class_addMethod cannot replace this complete NSObject deallocator.
        let added = unsafe {
            objc::class_addMethod(
                cls,
                objc::sel_reg_name(c"dealloc".as_ptr().cast()),
                std::mem::transmute::<
                    extern "C" fn(&mut CapturerCallbacks, &objc::Sel),
                    extern "C" fn(),
                >(dealloc),
                c"v@:".as_ptr().cast(),
            )
        };
        assert!(added, "Capture delegate deallocator already registered");
    }

    fn cls_add_protocol(_: &objc::Class<objc::Id>) {}
}

impl CapturerCallbacksDeallocation for CapturerCallbacks {}

unsafe impl Send for CapturerCallbacks {}
unsafe impl Sync for CapturerCallbacks {}

impl sc::stream::Output for CapturerCallbacks {}

#[objc::add_methods]
impl sc::stream::OutputImpl for CapturerCallbacks {
    extern "C" fn impl_stream_did_output_sample_buf(
        &mut self,
        _cmd: Option<&objc::Sel>,
        _: &sc::Stream,
        sample_buf: &mut cm::SampleBuf,
        kind: sc::OutputType,
    ) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(cb) = &mut self.inner_mut().did_output_sample_buf_cb {
                let sample_buf = sample_buf.retained();
                let frame = match kind {
                    // Klipolo: with or without an image; the status says which.
                    sc::OutputType::Screen => Frame::Screen(VideoFrame { sample_buf }),
                    sc::OutputType::Audio => Frame::Audio(AudioFrame { sample_buf }),
                    // Klipolo: no microphone output is ever added (the mic is its own pipe).
                    sc::OutputType::Mic => return,
                };
                (cb)(frame);
            }
        }));

        if result.is_err() {
            tracing::warn!("Suppressed panic in ScreenCaptureKit output callback");
        }
    }
}

impl sc::stream::Delegate for CapturerCallbacks {}

#[objc::add_methods]
impl sc::stream::DelegateImpl for CapturerCallbacks {
    extern "C" fn impl_stream_did_stop_with_err(
        &mut self,
        _cmd: Option<&objc::Sel>,
        stream: &sc::Stream,
        error: &ns::Error,
    ) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(cb) = &mut self.inner_mut().did_stop_with_err_cb {
                (cb)(stream, error)
            }
        }));

        if result.is_err() {
            tracing::warn!("Suppressed panic in ScreenCaptureKit stop callback");
        }
    }

    // Klipolo: the captured target went away (an app with no window left on the display, a window closed).
    extern "C" fn impl_stream_did_become_inactive(
        &mut self,
        _cmd: Option<&objc::Sel>,
        _stream: &sc::Stream,
    ) {
        self.active(false);
    }

    // Klipolo: and came back.
    extern "C" fn impl_stream_did_become_active(
        &mut self,
        _cmd: Option<&objc::Sel>,
        _stream: &sc::Stream,
    ) {
        self.active(true);
    }
}

impl CapturerCallbacks {
    /// Klipolo: the stream's activity, to `with_active_cb`'s callback.
    fn active(&mut self, active: bool) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(cb) = &mut self.inner_mut().did_change_activity_cb {
                (cb)(active)
            }
        }));

        if result.is_err() {
            tracing::warn!("Suppressed panic in ScreenCaptureKit activity callback");
        }
    }
}

type DidOutputSampleBufCallback = Box<dyn FnMut(Frame)>;
type StreamDidStopwithErrCallback = Box<dyn FnMut(&sc::Stream, &ns::Error)>;
type StreamDidChangeActivityCallback = Box<dyn FnMut(bool)>;

#[derive(Default)]
pub struct CapturerCallbacksInner {
    did_output_sample_buf_cb: Option<DidOutputSampleBufCallback>,
    did_stop_with_err_cb: Option<StreamDidStopwithErrCallback>,
    did_change_activity_cb: Option<StreamDidChangeActivityCallback>,
}

pub struct Capturer {
    _queue: arc::R<dispatch::Queue>,
    stream: arc::R<sc::Stream>,
    // READING THIS IS NOT THREAD SAFE, IT JUST HAS TO EXIST
    _callbacks: arc::R<CapturerCallbacks>,
}

// SAFETY (Klipolo): the stream may be started and stopped from any thread; the callbacks are only ever called on the
// capturer's own serial queue, and the struct only keeps them alive.
unsafe impl Send for Capturer {}

impl Capturer {
    pub fn builder(
        target: arc::R<sc::ContentFilter>,
        config: arc::R<sc::StreamCfg>,
    ) -> CapturerBuilder {
        CapturerBuilder {
            target,
            config,
            callbacks: CapturerCallbacksInner::default(),
        }
    }

    /// Klipolo: starts capturing, waiting at most `limit` for ScreenCaptureKit to answer; Err with its error, or None
    /// when it didn't answer in time.
    pub fn start_within(&self, limit: Duration) -> Result<(), Option<arc::R<ns::Error>>> {
        let (tx, rx) = sync_channel(1);
        self.stream.start_with_ch(move |error| {
            let _ = tx.try_send(error.map(|e| e.retained()));
        });
        match rx.recv_timeout(limit) {
            Ok(None) => Ok(()),
            Ok(Some(error)) => Err(Some(error)),
            Err(_) => Err(None),
        }
    }

    /// Klipolo: a handle that asks the stream to apply `cfg` again (`updateConfiguration:`), from any thread, and calls
    /// `answered` with the error's code when the stream answered with one, else None. A live stream answers, so a
    /// still screen can be told from a stuck stream.
    pub fn nudger(
        &self,
        cfg: arc::R<sc::StreamCfg>,
        answered: impl Fn(Option<isize>) + Send + Sync + 'static,
    ) -> impl Fn() + Send + Sync + 'static {
        type Answered = std::sync::Arc<dyn Fn(Option<isize>) + Send + Sync>;
        struct Nudge(arc::R<sc::Stream>, arc::R<sc::StreamCfg>, Answered);
        // SAFETY: SCStream's methods may be called from any thread; the configuration is only read by it.
        unsafe impl Send for Nudge {}
        unsafe impl Sync for Nudge {}
        let nudge = Nudge(self.stream.retained(), cfg, std::sync::Arc::new(answered));
        move || {
            // The whole handle, not its fields (a closure would capture those one by one).
            let nudge = &nudge;
            let answered = nudge.2.clone();
            nudge
                .0
                .update_cfg_ch(&nudge.1, move |error| answered(error.map(|e| e.code())))
        }
    }

    /// Klipolo: applies `cfg` (`updateConfiguration:`), waiting at most `limit` for the stream's answer; Err
    /// with its error's code, or None when it didn't answer in time.
    pub fn update_within(&self, cfg: &sc::StreamCfg, limit: Duration) -> Result<(), Option<isize>> {
        let (tx, rx) = sync_channel(1);
        self.stream.update_cfg_ch(cfg, move |error| {
            let _ = tx.try_send(error.map(|e| e.code()));
        });
        match rx.recv_timeout(limit) {
            Ok(None) => Ok(()),
            Ok(Some(code)) => Err(Some(code)),
            Err(_) => Err(None),
        }
    }

    /// Klipolo: stops capturing, waiting at most `limit`; false when it didn't answer in time or failed.
    pub fn stop_within(&self, limit: Duration) -> bool {
        let (tx, rx) = sync_channel(1);
        self.stream.stop_with_ch(move |error| {
            let _ = tx.try_send(error.is_none());
        });
        rx.recv_timeout(limit).unwrap_or(false)
    }
}

pub struct VideoFrame {
    sample_buf: arc::R<cm::SampleBuf>,
}

unsafe impl Send for VideoFrame {}

impl VideoFrame {
    pub fn sample_buf(&self) -> &cm::SampleBuf {
        self.sample_buf.as_ref()
    }
}

pub struct AudioFrame {
    sample_buf: arc::R<cm::SampleBuf>,
}

impl AudioFrame {
    pub fn sample_buf(&self) -> &cm::SampleBuf {
        self.sample_buf.as_ref()
    }
}

pub enum Frame {
    Screen(VideoFrame),
    Audio(AudioFrame),
}

pub struct CapturerBuilder {
    target: arc::R<sc::ContentFilter>,
    config: arc::R<sc::StreamCfg>,
    callbacks: CapturerCallbacksInner,
}

impl CapturerBuilder {
    pub fn with_output_sample_buf_cb(mut self, cb: impl FnMut(Frame) + 'static) -> Self {
        self.callbacks.did_output_sample_buf_cb = Some(Box::new(cb));
        self
    }

    pub fn with_stop_with_err_cb(
        mut self,
        cb: impl FnMut(&sc::Stream, &ns::Error) + 'static,
    ) -> Self {
        self.callbacks.did_stop_with_err_cb = Some(Box::new(cb));
        self
    }

    /// Klipolo: `cb(false)` when the stream became inactive (its target went away), `cb(true)` when it is active again.
    pub fn with_active_cb(mut self, cb: impl FnMut(bool) + 'static) -> Self {
        self.callbacks.did_change_activity_cb = Some(Box::new(cb));
        self
    }

    pub fn build(self) -> Result<Capturer, arc::R<ns::Error>> {
        let callbacks = CapturerCallbacks::with(self.callbacks);

        let stream = sc::Stream::with_delegate(&self.target, &self.config, callbacks.as_ref());

        // Klipolo: serial, an autorelease pool per item (as Cap's), at user-interactive QoS.
        let attrs = dispatch::Attr::make_with_qos_class(
            Some(&dispatch::Attr::serial_with_ar_pool()),
            dispatch::QosClass::USER_INTERACTIVE,
            0,
        );
        let queue =
            dispatch::Queue::with_label_and_attrs(Some(c"klipolo.capture.screen"), Some(&attrs));

        if self.config.captures_audio() {
            stream
                .add_stream_output(callbacks.as_ref(), sc::OutputType::Audio, Some(&queue))
                .map_err(|e| e.retained())?;
        }

        stream
            .add_stream_output(callbacks.as_ref(), sc::OutputType::Screen, Some(&queue))
            .map_err(|e| e.retained())?;

        Ok(Capturer {
            _queue: queue,
            stream,
            _callbacks: callbacks,
        })
    }
}

#[cfg(test)]
mod deallocation_tests {
    use super::*;
    use std::{
        ffi::c_void,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_setAssociatedObject"]
        fn set_associated_object(
            object: *const c_void,
            key: *const c_void,
            value: *const c_void,
            policy: usize,
        );
    }

    static ASSOCIATION_KEY: u8 = 0;

    struct DropCounter(Arc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn delegate(counter: DropCounter) -> arc::R<CapturerCallbacks> {
        CapturerCallbacks::with(CapturerCallbacksInner {
            did_output_sample_buf_cb: Some(Box::new(move |_| {
                let _ = &counter;
            })),
            did_stop_with_err_cb: None,
            did_change_activity_cb: None,
        })
    }

    #[test]
    fn final_release_drops_payload_and_native_associated_objects_once() {
        let drops = Arc::new(AtomicUsize::new(0));
        for cycle in 0..100 {
            let outer = delegate(DropCounter(drops.clone()));
            let associated = delegate(DropCounter(drops.clone()));
            unsafe {
                set_associated_object(
                    std::ptr::from_ref(outer.as_ref()).cast(),
                    std::ptr::from_ref(&ASSOCIATION_KEY).cast(),
                    std::ptr::from_ref(associated.as_ref()).cast(),
                    1,
                );
            }
            drop(associated);
            assert_eq!(drops.load(Ordering::SeqCst), cycle * 2);
            drop(outer);
            assert_eq!(drops.load(Ordering::SeqCst), (cycle + 1) * 2);
        }
    }
}
