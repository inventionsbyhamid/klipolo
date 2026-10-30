//! Thread quality-of-service classes, which steer core placement, timer latency and I/O priority. On macOS Apple asks
//! for `pthread_set_qos_class_self_np` rather than raw priorities (those lock a thread out of QoS). Windows (W:
//! EcoQoS/HighQoS through `SetThreadInformation`, MMCSS) and Linux (L: nice, ioprio, rtkit) don't set anything yet.
//!
//! Klipolo's map: the audio callback stays real-time (CoreAudio owns it); the mixer, the native presenter and the
//! preview decoders are user-interactive (the user is watching or listening); the export worker's threads are
//! user-initiated (a ceiling caps everything it starts); the PCM cache build, loudness measuring and
//! project saving are utility (background work the user isn't waiting on frame by frame), except a save someone waits
//! for (closing, quitting, the app going to the background), which runs at user-initiated, the class `lend` gives a
//! thread while another waits on it.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Qos {
    Utility = 1,
    UserInitiated = 2,
    UserInteractive = 3,
}

/// The highest class any thread of this process takes (0: none). The export worker caps itself at user-initiated,
/// so the decoders it starts don't compete with the editor's UI.
static CEILING: AtomicU8 = AtomicU8::new(0);

pub fn set_ceiling(qos: Qos) {
    CEILING.store(qos as u8, Ordering::SeqCst);
}

/// `qos`, capped by the ceiling.
fn capped(qos: Qos) -> Qos {
    match CEILING.load(Ordering::SeqCst) {
        2 if qos == Qos::UserInteractive => Qos::UserInitiated,
        1 => Qos::Utility,
        _ => qos,
    }
}

/// Sets the calling thread's QoS class, capped by the ceiling (a no-op off macOS).
pub fn set_current(qos: Qos) {
    crate::imp::set_qos(capped(qos));
}

/// A class lent to another thread (`lend`), until this is dropped.
#[must_use = "the class is lent only while this lives"]
pub struct Lent(#[allow(dead_code)] crate::imp::Lent);

/// Lends `qos` (capped by the ceiling) to `thread` while the returned `Lent` lives: a caller about to wait on work
/// that thread does raises it, so that work isn't held back at a lower class, by the CPU or by the disk's throttling
/// of background I/O (a close's flush waiting on the utility save thread during an export). The thread keeps its own
/// class if that is higher. macOS: a QoS override (`pthread_override_qos_class_start_np`); W and L: nothing yet.
pub fn lend<T>(thread: &std::thread::JoinHandle<T>, qos: Qos) -> Lent {
    Lent(crate::imp::lend(thread, capped(qos)))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// The calling thread's base scheduling priority as the kernel has it now (utility 20, user-initiated 37).
    fn base_priority() -> i32 {
        // SAFETY: plain data, filled by `thread_info`.
        let mut info: libc::thread_extended_info = unsafe { std::mem::zeroed() };
        let mut count = libc::THREAD_EXTENDED_INFO_COUNT;
        // SAFETY: asks about this thread (its port, borrowed from its pthread) into `info`, `count` words long.
        let kr = unsafe {
            libc::thread_info(
                libc::pthread_mach_thread_np(libc::pthread_self()),
                libc::THREAD_EXTENDED_INFO as libc::thread_flavor_t,
                (&raw mut info).cast(),
                &mut count,
            )
        };
        assert_eq!(kr, 0, "thread_info");
        info.pth_priority
    }

    #[test]
    fn a_lent_class_raises_the_thread_only_while_it_lives() {
        let (ask, asked) = mpsc::channel::<()>();
        let (tell, told) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            set_current(Qos::Utility);
            for () in asked {
                tell.send(base_priority()).unwrap();
            }
        });
        let priority = || {
            ask.send(()).unwrap();
            told.recv().unwrap()
        };
        let alone = priority();
        let own = std::thread::spawn(|| {
            set_current(Qos::UserInitiated);
            base_priority()
        })
        .join()
        .unwrap();
        let lent = lend(&thread, Qos::UserInitiated);
        let raised = priority();
        drop(lent);
        let after = priority();
        drop(ask);
        thread.join().unwrap();
        println!(
            "base priority: utility {alone}, lent user-initiated {raised}, after {after}; user-initiated itself {own}"
        );
        assert!(raised > alone, "utility {alone}, lent {raised}");
        assert_eq!(raised, own, "as high as the class itself");
        assert_eq!(after, alone, "back once the loan ends");
    }
}
