//! The probes the sandboxed child (`child.rs`) runs: each answers Ok or the refusal.

use std::ffi::{CStr, c_char};
use std::io::Read;
use std::path::Path;

/// `sandbox_check`'s filter types (libsandbox's `enum sandbox_filter_type`, as WebKit's `SandboxSPI.h` declares it).
pub(super) const GLOBAL_NAME: libc::c_int = 2;
pub(super) const XPC_SERVICE_NAME: libc::c_int = 12;

/// Whether this process's sandbox allows `operation` on `name` (EPERM: refused; an unsandboxed process: allowed).
pub(super) fn policy(operation: &CStr, filter: libc::c_int, name: &CStr) -> std::io::Result<()> {
    unsafe extern "C" {
        fn sandbox_check(
            pid: libc::pid_t,
            operation: *const c_char,
            filter: libc::c_int,
            ...
        ) -> libc::c_int;
    }
    // SAFETY: NUL-terminated strings, as the filter type expects one name.
    match unsafe { sandbox_check(libc::getpid(), operation.as_ptr(), filter, name.as_ptr()) } {
        0 => Ok(()),
        1 => Err(std::io::Error::from_raw_os_error(libc::EPERM)),
        other => Err(std::io::Error::other(format!("sandbox_check: {other}"))),
    }
}

/// `task_name_for_pid` (or `task_for_pid`) of `pid`: the port, released at once, or the kernel's refusal.
pub(super) fn task_port(pid: libc::pid_t, name_only: bool) -> std::io::Result<()> {
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_name_for_pid(target: u32, pid: libc::c_int, port: *mut u32) -> libc::c_int;
        fn task_for_pid(target: u32, pid: libc::c_int, port: *mut u32) -> libc::c_int;
        fn mach_port_deallocate(task: u32, name: u32) -> libc::c_int;
    }
    let mut port = 0u32;
    // SAFETY: an out-pointer for one port name, which is released below when the call gave one.
    let result = unsafe {
        if name_only {
            task_name_for_pid(mach_task_self_, pid, &mut port)
        } else {
            task_for_pid(mach_task_self_, pid, &mut port)
        }
    };
    if result != 0 {
        return Err(std::io::Error::other(format!("kern_return {result}")));
    }
    // SAFETY: the port name the call returned, released once.
    unsafe { mach_port_deallocate(mach_task_self_, port) };
    Ok(())
}

pub(super) fn is_open(fd: libc::c_int) -> bool {
    // SAFETY: reads a descriptor's flags; EBADF when it isn't open.
    unsafe { libc::fcntl(fd, libc::F_GETFD) != -1 }
}

/// A UDP socket's `connect` to `ip:port`: the sandbox's network check, with no packet sent.
pub(super) fn udp_connect(ip: [u8; 4], port: u16) -> std::io::Result<()> {
    // SAFETY: a socket this function owns and closes, and a sockaddr_in it fills in.
    unsafe {
        let socket = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if socket < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
        addr.sin_family = libc::AF_INET as u8;
        addr.sin_port = port.to_be();
        addr.sin_addr.s_addr = u32::from_be_bytes(ip).to_be();
        let done = libc::connect(socket, (&raw const addr).cast(), u32::from(addr.sin_len));
        let error = std::io::Error::last_os_error();
        libc::close(socket);
        if done == 0 { Ok(()) } else { Err(error) }
    }
}

/// The first bytes of `file`.
pub(super) fn head(file: &Path) -> std::io::Result<()> {
    std::fs::File::open(file)?.read_exact(&mut [0; 16])
}

/// The first bytes of the first font file in `dir`, if it has one.
pub(super) fn first_file_in(dir: &Path) -> std::io::Result<()> {
    let font = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.extension().is_some_and(|e| {
                ["ttf", "ttc", "otf"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            })
        });
    // A folder with no font file: listing it was the read.
    font.map_or(Ok(()), |font| head(&font))
}

/// `lchflags(path, flags)`.
pub(super) fn set_flags(path: &Path, flags: u32) -> std::io::Result<()> {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a NUL-terminated path.
    if unsafe { libc::lchflags(path.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Sets an empty extended ACL on `path` (`acl_set_link_np`): a change of its ACL that leaves it as it was.
pub(super) fn clear_acl(path: &Path) -> std::io::Result<()> {
    unsafe extern "C" {
        fn acl_init(count: libc::c_int) -> *mut libc::c_void;
        fn acl_set_link_np(
            path: *const c_char,
            kind: libc::c_int,
            acl: *mut libc::c_void,
        ) -> libc::c_int;
        fn acl_free(object: *mut libc::c_void) -> libc::c_int;
    }
    const ACL_TYPE_EXTENDED: libc::c_int = 0x100;
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: an empty ACL this function frees, and a NUL-terminated path.
    unsafe {
        let acl = acl_init(0);
        if acl.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let done = acl_set_link_np(path.as_ptr(), ACL_TYPE_EXTENDED, acl);
        let error = std::io::Error::last_os_error();
        acl_free(acl);
        if done == 0 { Ok(()) } else { Err(error) }
    }
}
