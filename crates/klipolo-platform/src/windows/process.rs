//! Processes on Windows: whether stdin is the parent's pipe, another process's state, memory, start and executable,
//! stopping one (later), and the flags the export and recorder workers start with.

use std::io;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, HANDLE};

pub(crate) fn stdin_is_pipe() -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType};
    let handle = std::io::stdin().as_raw_handle() as HANDLE;
    // SAFETY: asks the type of this process's own stdin handle.
    unsafe { GetFileType(handle) == FILE_TYPE_PIPE }
}

/// `OpenProcess` for a status query: a process that is gone can't be opened, or has an exit code.
pub(crate) fn is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a query-only handle to the process, closed before returning; the exit code goes into a local.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // It exists but isn't ours to open.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let running = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        running
    }
}

/// This process's footprint (`process_memory`): the anonymous memory a bench counts.
pub(crate) fn own_anonymous_memory() -> Option<u64> {
    process_memory(std::process::id()).map(|m| m.footprint)
}

/// `K32GetProcessMemoryInfo`: the private commit (`PrivateUsage`: Task Manager's "Commit size"; its "Memory" column
/// is the private working set) and the working set. None when the process can't be opened or has exited. The handle
/// asks for `PROCESS_VM_READ` too, as MSDN lists it beside the query right.
pub(crate) fn process_memory(pid: u32) -> Option<crate::process::ProcessMemory> {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: a query handle to the process, closed before returning; the exit code goes into a local and the
    // counters into a struct whose size is passed with it (the EX form, which `cb` announces).
    let ok = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0
            && code == STILL_ACTIVE as u32
            && K32GetProcessMemoryInfo(
                handle,
                (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
                counters.cb,
            ) != 0;
        CloseHandle(handle);
        ok
    };
    ok.then_some(crate::process::ProcessMemory {
        footprint: counters.PrivateUsage as u64,
        resident: counters.WorkingSetSize as u64,
    })
}

/// Later: `GetProcessTimes`' creation time (a FILETIME, stable for the process's life), as µs since 1970. Unknown until
/// then, so nothing matches a pidfile and a sweep stops nothing.
pub(crate) fn start_time(_pid: u32) -> Option<u64> {
    None
}

/// Later: `GetProcessTimes` against `started`. Never, until then (no start time to match).
pub(crate) fn still_running(_pid: u32, _started: u64) -> bool {
    false
}

/// Later: `OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION)`, `GetProcessTimes` against `started`,
/// then `TerminateProcess` on the same handle, so the check can't race (sidecars live in a Job Object with
/// `KILL_ON_JOB_CLOSE`, so they also end with the app).
pub(crate) fn terminate(_pid: u32, _started: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stopping a process by id comes with the Windows build (TerminateProcess)",
    ))
}

/// Later: `TerminateProcess` on a handle whose `GetProcessTimes` matches `started`.
pub(crate) fn kill(_pid: u32, _started: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stopping a process by id comes with the Windows build (TerminateProcess)",
    ))
}

/// `QueryFullProcessImageNameW` on a query handle. None when the process can't be opened.
pub(crate) fn exe_path(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: a query-only handle, closed before returning; the buffer's length is passed and updated by the call.
    let ok = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let ok =
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(handle);
        ok
    };
    ok.then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len as usize])))
}

/// Later: on a handle whose `GetProcessTimes` matches `started`, `SetPriorityClass(IDLE_PRIORITY_CLASS)` (the
/// background mode only applies to the calling process) and EcoQoS through
/// `SetProcessInformation(ProcessPowerThrottling)`. Nothing yet.
pub(crate) fn set_background(_pid: u32, _started: u64, _on: bool) -> io::Result<()> {
    Ok(())
}

/// The boot time from `NtQuerySystemInformation(SystemTimeOfDayInformation)`, in 100 ns since 1601. The struct is
/// opaque in `winternl.h`; its first field has been the boot time since NT 4 (Process Hacker and others read it).
/// Untried on Windows so far: if the call fails there is no boot id, so nothing matches a pidfile.
pub(crate) fn boot_id() -> Option<String> {
    use windows_sys::Wdk::System::SystemInformation::{
        NtQuerySystemInformation, SystemTimeOfDayInformation,
    };
    // `SYSTEM_TIMEOFDAY_INFORMATION`: 48 bytes, the boot time first.
    let mut info = [0i64; 6];
    let mut written = 0u32;
    // SAFETY: the class's out-struct, with its size passed; the call writes at most that many bytes.
    let status = unsafe {
        NtQuerySystemInformation(
            SystemTimeOfDayInformation,
            info.as_mut_ptr().cast(),
            std::mem::size_of_val(&info) as u32,
            &mut written,
        )
    };
    (status >= 0 && written >= 8 && info[0] > 0).then(|| info[0].to_string())
}

/// Debug builds are console programs: a worker would open its own console window without this.
pub(crate) fn worker_flags(command: &mut tokio::process::Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `process::recorder_command`'s rule: out of the app's console group and any job that would end it with the app, and
/// no console window.
pub(crate) fn recorder_flags(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW);
}

/// `recorder_flags` without `CREATE_BREAKAWAY_FROM_JOB`, for an app in a job that forbids it.
pub(crate) fn recorder_flags_in_job(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

/// Windows has no stop request for a windowless process: `process::stop_child`'s hard stop follows.
pub(crate) fn ask_to_stop(_child: &std::process::Child) -> io::Result<()> {
    Ok(())
}

/// Later: a Toolhelp snapshot (`CreateToolhelp32Snapshot`) with `QueryFullProcessImageNameW` for each process, and its
/// mode from the command line. `Unsupported` until then (never "nothing runs"), and `install::place` answers Unknown on
/// Windows.
pub(crate) fn running_from(_bundle: &std::path::Path) -> io::Result<Vec<crate::process::Running>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the processes running from the bundle can't be listed here yet",
    ))
}
