//! A speech worker's priority: below normal for good, through the
//! OS's own calls only, so `klipolo-asr` lowers itself exactly as whisper's worker does without linking klipolo-platform.
//! `klipolo_platform::process::lower_self` calls `lower`: there is one implementation.
//!
//! Playback is protected by the speech driver's hold on late frames, not by starving the worker: at the OS's lowest
//! priority every editor thread preempted the worker's threads, and the others spun waiting for them.

use std::io;

/// The nice value `lower` gives: below normal, as utility work is, never the OS's lowest.
pub const LOWERED_NICE: i32 = 10;

/// This process below normal CPU priority, for good, before it starts any other thread (so they all inherit it):
/// - macOS: nice `LOWERED_NICE` for the process (on macOS nice is the task's: every thread, now and later) and utility
///   QoS for the calling thread, which the threads it starts inherit; never the darwin background band, which also
///   throttles disk reads;
/// - Linux: nice `LOWERED_NICE` (or nicer, if it already is) and `SCHED_BATCH` for the calling thread, which the
///   threads it starts inherit (raising nice back needs `CAP_SYS_NICE`, hence for good); never `SCHED_IDLE`, under
///   which any other thread preempts a worker thread and ggml's other threads spin waiting for it. A kernel or sandbox
///   that refuses `SCHED_BATCH` leaves the nice value, said on stderr;
/// - Windows: `BELOW_NORMAL_PRIORITY_CLASS` for the process.
pub fn lower() -> io::Result<()> {
    imp::lower()
}

/// The highest class the calling thread (and so every thread it starts) runs at while the worker loads: at most
/// user-initiated, the libc twin of `klipolo_platform::qos::set_ceiling(Qos::UserInitiated)`. macOS: a thread at
/// user-interactive is moved down to user-initiated, and a lower or unspecified class is left alone. Windows and Linux
/// have no such class (as `klipolo_platform::qos`): nothing.
pub fn ceiling() {
    imp::ceiling();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::io;

    use libc::qos_class_t::{QOS_CLASS_UNSPECIFIED, QOS_CLASS_USER_INITIATED, QOS_CLASS_UTILITY};

    pub fn lower() -> io::Result<()> {
        // SAFETY: changes this process's own nice value (`who` 0 is the caller).
        if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, super::LOWERED_NICE) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: only changes the calling thread's own scheduling class.
        let _ = unsafe { libc::pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0) };
        Ok(())
    }

    pub fn ceiling() {
        let mut class = QOS_CLASS_UNSPECIFIED;
        let mut relative = 0;
        // SAFETY: reads the calling thread's own class into two locals; the kernel writes one of `qos_class_t`'s
        // values.
        let read = unsafe {
            libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative)
        };
        if read == 0 && (class as u32) > (QOS_CLASS_USER_INITIATED as u32) {
            // SAFETY: only changes the calling thread's own scheduling class.
            let _ = unsafe { libc::pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0) };
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io;

    pub fn lower() -> io::Result<()> {
        let nice = super::LOWERED_NICE;
        // SAFETY: reads the calling thread's own nice value (`who` 0); an error reads as -1, below `nice`, so it is
        // set.
        if unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) } < nice
            // SAFETY: changes the calling thread's own nice value (`who` 0).
            && unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let param = libc::sched_param { sched_priority: 0 };
        // SAFETY: the calling thread's own policy (`pid` 0), with a parameter that outlives the call.
        if unsafe { libc::sched_setscheduler(0, libc::SCHED_BATCH, &param) } != 0 {
            eprintln!(
                "speech worker: SCHED_BATCH refused ({}); the nice value stays",
                io::Error::last_os_error()
            );
        }
        Ok(())
    }

    pub fn ceiling() {}
}

#[cfg(windows)]
mod imp {
    use std::io;

    pub fn lower() -> io::Result<()> {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
        };
        // SAFETY: this process's pseudo-handle (never closed) and a priority class.
        if unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn ceiling() {}
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use std::io;

    pub fn lower() -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn ceiling() {}
}
