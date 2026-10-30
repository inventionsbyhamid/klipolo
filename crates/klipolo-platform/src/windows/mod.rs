//! The Windows backend (until W): the clock, durable files and process plumbing are simple and real; QoS, power,
//! memory pressure, the audio route and the seams updates ask about (`updates`) are stubs that change nothing (each
//! names its W counterpart).

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL, ERROR_LOCK_VIOLATION,
    ERROR_SHARING_VIOLATION, ERROR_WRITE_PROTECT, HANDLE,
};

pub(crate) mod fonts;
mod process;
pub(crate) mod sandbox;
mod trash;
mod updates;

pub(crate) use process::{
    ask_to_stop, boot_id, exe_path, is_alive, kill, own_anonymous_memory, process_memory,
    recorder_flags, recorder_flags_in_job, running_from, set_background, start_time, stdin_is_pipe,
    still_running, terminate, worker_flags,
};
pub(crate) use trash::{move_to_trash, put_back};
pub(crate) use updates::{
    bundle, install_place, low_power, move_to_applications, watch_net_path, watch_wake,
};

use crate::durable::{DiskReason, Durability};
use crate::qos::Qos;

#[cfg(any(test, feature = "fake"))]
pub(crate) const DISK_FULL: i32 = ERROR_DISK_FULL as i32;

/// `QueryPerformanceCounter` in ns.
pub(crate) fn host_ns() -> u64 {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    static FREQUENCY: OnceLock<u64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: writes the counter's frequency into a local.
        unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1) as u64
    });
    let mut count = 0i64;
    // SAFETY: writes the counter into a local.
    unsafe { QueryPerformanceCounter(&mut count) };
    (u128::from(count.max(0) as u64) * 1_000_000_000 / u128::from(frequency)) as u64
}

/// W: EcoQoS for utility work and HighQoS for the export worker (`SetThreadInformation`), MMCSS for audio.
pub(crate) fn set_qos(_qos: Qos) {}

/// W: raising the waited-on thread (`SetThreadPriority` boost, or EcoQoS off) while the caller waits.
pub(crate) struct Lent;

pub(crate) fn lend<T>(_thread: &std::thread::JoinHandle<T>, _qos: Qos) -> Lent {
    Lent
}

/// W: `PowerCreateRequest` + `PowerSetRequest(SystemRequired | ExecutionRequired)`.
pub struct Activity;

pub(crate) fn begin_activity(_reason: &str) -> Activity {
    Activity
}

/// Later: `PowerSetRequest` System, Display and ExecutionRequired for the take. Until then, `begin_activity`'s.
pub(crate) fn begin_capture(reason: &str) -> Activity {
    begin_activity(reason)
}

/// W: the effective power mode (`PowerRegisterForEffectivePowerModeNotifications`), battery saver as `Fair`.
pub(crate) fn thermal() -> crate::power::Thermal {
    crate::power::Thermal::Nominal
}

/// W: `CreateMemoryResourceNotification` and the DXGI budget. The level stays Normal.
pub(crate) fn watch_pressure() {}

/// W: `IMMNotificationClient::OnDefaultDeviceChanged`. Reports no changes.
#[derive(Default)]
pub struct RouteWatch;

impl RouteWatch {
    pub fn new() -> Self {
        Self
    }

    pub fn follow(&mut self) {}
}

/// W: `IAudioClient::GetStreamLatency` and `IAudioClock`.
pub(crate) fn hardware_latency() -> Duration {
    Duration::ZERO
}

/// W: the webview's `EyeDropper` (WebView2 has one) rather than this seam. None until then: the UI shows no eyedropper.
pub(crate) fn screen_color_supported() -> bool {
    false
}

pub(crate) fn sample_screen_color(done: Box<dyn FnOnce(Option<String>) + Send>) {
    done(None)
}

/// `FlushFileBuffers` either way (W: `MoveFileExW(WRITE_THROUGH)` and retries on antivirus locks).
pub(crate) fn sync(file: &File, durability: Durability) -> io::Result<()> {
    match durability {
        Durability::Ordered => file.sync_data(),
        Durability::Full => file.sync_all(),
    }
}

/// `FlushFileBuffers`, which flushes the drive's cache too.
pub(crate) fn push(file: &File) -> io::Result<()> {
    file.sync_data()
}

/// Nothing: `push` already reached the disk.
pub(crate) fn order(_file: &File) -> io::Result<()> {
    Ok(())
}

/// W: `GetVolumeInformationByHandleW`'s volume serial number. Unknown until then.
pub(crate) fn volume_id(_path: &Path) -> Option<u64> {
    None
}

/// W: the volume serial and file index of a handle opened with backup semantics. Unknown until then.
pub(crate) fn placed_identity(_path: &Path) -> Option<crate::file::PlacedIdentity> {
    None
}

/// NTFS journals renames; there is no folder to flush.
pub(crate) fn fsync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// No Trash folder by path: the Recycle Bin is known by its name on each drive.
pub(crate) fn trash_roots() -> Vec<PathBuf> {
    Vec::new()
}

/// `$Recycle.Bin`, at each drive's root.
pub(crate) fn is_trash_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.eq_ignore_ascii_case("$Recycle.Bin"))
}

/// W: a handle opened with `FILE_FLAG_BACKUP_SEMANTICS` and every share mode, answering `GetFinalPathNameByHandleW`
/// (`folder::HeldFolder`). None yet: folders are followed by their path.
pub(crate) enum Held {}

pub(crate) fn hold_folder(_path: &Path) -> io::Result<Option<Held>> {
    Ok(None)
}

pub(crate) fn locate_folder(held: &Held) -> Option<PathBuf> {
    match *held {}
}

/// Win32 error codes.
pub(crate) fn classify_os(code: i32) -> DiskReason {
    match code as u32 {
        ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL => DiskReason::Full,
        ERROR_WRITE_PROTECT => DiskReason::ReadOnly,
        ERROR_ACCESS_DENIED => DiskReason::Denied,
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => DiskReason::Locked,
        _ => DiskReason::Other,
    }
}

/// The C runtime's errno values (`errno.h`), which FFmpeg's `AVERROR` carries.
pub(crate) fn classify_errno(errno: i32) -> DiskReason {
    const EPERM: i32 = 1;
    const EACCES: i32 = 13;
    const EFBIG: i32 = 27;
    const ENOSPC: i32 = 28;
    const EROFS: i32 = 30;
    match errno {
        ENOSPC | EFBIG => DiskReason::Full,
        EROFS => DiskReason::ReadOnly,
        EACCES | EPERM => DiskReason::Denied,
        _ => DiskReason::Other,
    }
}

/// `GetDiskFreeSpaceExW`: bytes available to this user.
pub(crate) fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: a NUL-terminated wide path; the totals we don't need are null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(available)
}

/// `seek_write` until all is written (it may write less, and it moves the cursor).
pub(crate) fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `seek_read` until `buf` is full.
pub(crate) fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// W: `PrefetchVirtualMemory` over the range. Nothing yet: the pages come in as they are read.
pub(crate) fn will_need(_bytes: &[u8]) {}

/// `GetFileInformationByHandle`: the volume serial number and the 64-bit file index.
pub(crate) fn file_identity(file: &File) -> Option<crate::file::FileIdentity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the open file's handle, and an out-struct for the call to fill.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) } != 0;
    ok.then(|| crate::file::FileIdentity {
        volume: u64::from(info.dwVolumeSerialNumber),
        file: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

/// `GlobalMemoryStatusEx`'s total physical memory (0 when it can't be read).
pub(crate) fn memory_bytes() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: a struct with its length set, as the call asks.
    if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
        status.ullTotalPhys
    } else {
        0
    }
}

/// The Videos known folder (`FOLDERID_Videos`).
pub(crate) fn videos_dir() -> Option<PathBuf> {
    dirs::video_dir()
}

/// `%LOCALAPPDATA%\Klipolo\Logs` (`FOLDERID_LocalAppData`).
pub(crate) fn logs_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|local| local.join("Klipolo").join("Logs"))
}

/// The machine's own architecture from `IsWow64Process2` (an x64 build on Windows on Arm runs emulated).
pub(crate) fn cpu_arch() -> String {
    use windows_sys::Win32::System::SystemInformation::{
        IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
    let (mut process, mut native) = (0u16, 0u16);
    // SAFETY: this process's pseudo-handle and two locals for the machine types.
    let ok = unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, &mut native) } != 0;
    match native {
        IMAGE_FILE_MACHINE_ARM64 if ok => "aarch64".into(),
        IMAGE_FILE_MACHINE_AMD64 if ok => "x86_64".into(),
        _ => std::env::consts::ARCH.into(),
    }
}

/// "major.minor.build" from `RtlGetVersion` (`GetVersionEx` reports 6.2 to unmanifested programs).
pub(crate) fn os_version() -> String {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    // SAFETY: a zeroed struct with its size set, as RtlGetVersion asks.
    unsafe {
        let mut info: OSVERSIONINFOW = std::mem::zeroed();
        info.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        if RtlGetVersion(&mut info) != 0 {
            return String::new();
        }
        format!(
            "{}.{}.{}",
            info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
        )
    }
}

/// The Dock menu's counterpart (the taskbar's thumbnail toolbar, W: `dock`'s module doc) isn't built yet.
pub(crate) fn install_dock_menu(_items: crate::dock::DockItems, _click: crate::dock::DockClick) {
    tracing::debug!("dock: no app icon menu on this OS yet");
}
