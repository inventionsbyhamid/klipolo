//! The memory pressure dispatch source (moved from `klipolo-engine/src/render/pressure.rs`).

use cidre::dispatch;

extern "C-unwind" fn on_event(source: *mut dispatch::Src) {
    // SAFETY: the context is the source itself, which lives for the rest of the process.
    let flags = unsafe { &*source }.data();
    let level = if flags & dispatch::SourceMemoryPressureFlags::CRITICAL.0 != 0 {
        2
    } else if flags & dispatch::SourceMemoryPressureFlags::WARN.0 != 0 {
        1
    } else {
        0
    };
    crate::pressure::set_level(level);
    tracing::info!(level, "memory pressure");
}

pub(crate) fn start() {
    let queue = dispatch::Queue::global_with_qos(dispatch::QosClass::UTILITY);
    let flags = dispatch::SourceMemoryPressureFlags::NORMAL
        | dispatch::SourceMemoryPressureFlags::WARN
        | dispatch::SourceMemoryPressureFlags::CRITICAL;
    let Some(mut source) = dispatch::Src::new_memory_pressure(flags, queue.map(|q| &**q)) else {
        tracing::warn!("no memory pressure source");
        return;
    };
    let raw: *mut dispatch::Src = &mut *source;
    source.set_context(raw.cast());
    source.set_event_handler_f(Some(on_event as dispatch::Fn<dispatch::Src>));
    source.activate();
    // Listens for the life of the process.
    std::mem::forget(source);
}
