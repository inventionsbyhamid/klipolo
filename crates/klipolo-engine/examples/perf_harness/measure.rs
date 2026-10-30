//! What the harness measures: Rust heap allocations (a counting allocator), process CPU time and footprint
//! (`proc_pid_rusage`), the VideoToolbox decoder services' footprint, and per-scenario reports.

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use super::Renders;

// ---------------------------------------------------------------------------------------------------------------
// Counting allocator: every Rust heap allocation in the process.

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: AllocLayout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

pub fn allocs() -> (u64, u64) {
    (
        ALLOCS.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------------------------------------------
// proc_pid_rusage (RUSAGE_INFO_V4): footprint, CPU time. macOS only (libproc): the harness measures the Mac, and
// elsewhere a process has no reading yet (W: GetProcessMemoryInfo and GetProcessTimes; L: /proc/<pid>/status and
// stat), so the CPU and footprint columns read zero there.

#[cfg(target_os = "macos")]
#[repr(C)]
struct RusageInfo {
    uuid: [u8; 16],
    f: [u64; 40],
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfo) -> i32;
}

#[derive(Clone, Copy, Default)]
pub struct Usage {
    pub cpu_ns: u64,
    pub footprint: u64,
    pub peak_footprint: u64,
}

#[cfg(target_os = "macos")]
pub fn usage(pid: i32) -> Option<Usage> {
    let mut info = RusageInfo {
        uuid: [0; 16],
        f: [0; 40],
    };
    // SAFETY: the buffer is larger than rusage_info_v4.
    if unsafe { proc_pid_rusage(pid, 4, &mut info) } != 0 {
        return None;
    }
    // Intel: mach absolute time is nanoseconds.
    Some(Usage {
        cpu_ns: info.f[0] + info.f[1],
        footprint: info.f[7],
        peak_footprint: info.f[28],
    })
}

#[cfg(not(target_os = "macos"))]
pub fn usage(_pid: i32) -> Option<Usage> {
    None
}

pub fn me() -> Usage {
    usage(std::process::id() as i32).unwrap_or_default()
}

/// Every VTDecoderXPCService's usage (VideoToolbox decodes out of process).
fn vt_usage() -> Vec<Usage> {
    let Ok(out) = std::process::Command::new("pgrep")
        .args(["-x", "VTDecoderXPCService"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .filter_map(usage)
        .collect()
}

/// Current footprint of every VTDecoderXPCService.
pub fn vt_footprint() -> u64 {
    vt_usage().iter().map(|u| u.footprint).sum()
}

/// CPU time every VTDecoderXPCService has used (ns): the decode work done for this process (and any other).
pub fn vt_cpu_ns() -> u64 {
    vt_usage().iter().map(|u| u.cpu_ns).sum()
}

pub fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

// ---------------------------------------------------------------------------------------------------------------

pub fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

pub struct Mark {
    at: Instant,
    allocs: (u64, u64),
    usage: Usage,
}

pub fn mark() -> Mark {
    Mark {
        at: Instant::now(),
        allocs: allocs(),
        usage: me(),
    }
}

pub fn report(name: &str, start: &Mark, stats: &Arc<Mutex<Renders>>, frames: usize) {
    let end = mark();
    let s = std::mem::take(&mut *stats.lock().unwrap_or_else(PoisonError::into_inner));
    let mut ms = s.ms.clone();
    ms.sort_by(f64::total_cmp);
    let n = frames.max(1) as f64;
    let (a, b) = (end.allocs.0 - start.allocs.0, end.allocs.1 - start.allocs.1);
    println!(
        "{name}: {} frames in {:.1} s | render ms p50 {:.2} p95 {:.2} max {:.2} | allocs/frame {:.0} \
         (backend {:.0}) | alloc KB/frame {:.0} | CPU ms/frame {:.2} | footprint {:.0} MB (peak {:.0} MB) | \
         VT XPC {:.0} MB",
        ms.len(),
        end.at.duration_since(start.at).as_secs_f64(),
        pct(&ms, 0.5),
        pct(&ms, 0.95),
        ms.last().copied().unwrap_or(0.0),
        a as f64 / n,
        s.backend_allocs as f64 / n,
        b as f64 / n / 1024.0,
        (end.usage.cpu_ns - start.usage.cpu_ns) as f64 / 1e6 / n,
        mb(end.usage.footprint),
        mb(end.usage.peak_footprint),
        mb(vt_footprint()),
    );
    let mut cuts = s.cuts;
    if !cuts.is_empty() {
        cuts.sort_by(f64::total_cmp);
        println!(
            "  clip cuts: {} frames, render ms p50 {:.1} max {:.1}",
            cuts.len(),
            pct(&cuts, 0.5),
            cuts.last().copied().unwrap_or(0.0)
        );
    }
}
