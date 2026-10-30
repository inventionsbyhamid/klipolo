//! The render worker's Seatbelt sandbox: `process::sandbox::PROFILE` with its parameters, applied to
//! this process for good.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use crate::process::sandbox::{Applied, PROFILE, SandboxProfile, escaped_profile, parameters};

/// `sandbox_init_with_parameters(profile, flags, parameters, errorbuf)`: exported by libsystem_sandbox but in no
/// header; Chromium's `sandbox/mac/seatbelt.cc` declares the same. `parameters` alternate names and values and end
/// with a null.
type InitWithParameters = unsafe extern "C" fn(
    profile: *const c_char,
    flags: u64,
    parameters: *const *const c_char,
    errorbuf: *mut *mut c_char,
) -> c_int;

// <sandbox.h>: deprecated in the SDK, present in every macOS. `flags` 0: `profile` is profile text.
unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

pub(crate) fn sandbox_self(profile: &SandboxProfile) -> Result<Applied, String> {
    let parameters = parameters(profile)?;
    match with_parameters() {
        Some(init) => apply_with_parameters(init, &parameters)?,
        None => apply_escaped(&parameters)?,
    }
    Ok(Applied::Sandboxed)
}

/// `sandbox_self` through `sandbox_init` and the escaped text, also where `sandbox_init_with_parameters` exists.
pub(crate) fn sandbox_self_escaped(profile: &SandboxProfile) -> Result<Applied, String> {
    apply_escaped(&parameters(profile)?)?;
    Ok(Applied::Sandboxed)
}

pub(crate) fn close_inherited() {
    crate::unix::close_above_stderr("/dev/fd");
}

pub(crate) fn limit_file_size(bytes: u64) -> std::io::Result<()> {
    crate::unix::limit_file_size(bytes)
}

pub(crate) fn raise_open_files(target: u64) -> std::io::Result<u64> {
    crate::unix::raise_open_files(target)
}

/// `getppid()`: the process that started this one, or what adopted it once that exited (launchd, init, a subreaper).
pub(crate) fn parent_id() -> Option<u32> {
    // SAFETY: getppid takes nothing and can't fail.
    Some(unsafe { libc::getppid() } as u32)
}

/// This process's file size limit (`RLIMIT_FSIZE`, soft), None when unlimited or unknown.
pub(crate) fn file_size_limit() -> Option<u64> {
    // SAFETY: a plain out-struct of two integers, written by getrlimit.
    let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: a valid resource and a pointer to that struct.
    let ok = unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &raw mut r) } == 0;
    (ok && r.rlim_cur != libc::RLIM_INFINITY).then_some(r.rlim_cur)
}

/// `path`'s BSD flags cleared and its extended ACL emptied (neither follows a link), then the owner's access on a
/// folder.
pub(crate) fn unlock(path: &std::path::Path) {
    unsafe extern "C" {
        fn acl_init(count: c_int) -> *mut c_void;
        fn acl_set_link_np(path: *const c_char, kind: c_int, acl: *mut c_void) -> c_int;
        fn acl_free(object: *mut c_void) -> c_int;
    }
    // <sys/acl.h>
    const ACL_TYPE_EXTENDED: c_int = 0x100;
    use std::os::unix::ffi::OsStrExt;
    if let Ok(text) = CString::new(path.as_os_str().as_bytes()) {
        // SAFETY: a NUL-terminated path; `lchflags` and `acl_set_link_np` act on the link itself, never what it names;
        // the empty ACL is freed here.
        unsafe {
            libc::lchflags(text.as_ptr(), 0);
            let acl = acl_init(0);
            if !acl.is_null() {
                acl_set_link_np(text.as_ptr(), ACL_TYPE_EXTENDED, acl);
                acl_free(acl);
            }
        }
    }
    crate::unix::owner_all(path);
}

/// This user's cache folder, `$DARWIN_USER_CACHE_DIR` (`confstr(_CS_DARWIN_USER_CACHE_DIR)`, which asks the system for
/// the user's folder and reads no environment variable), resolved: where Metal keeps its shader cache. None when the
/// system names none.
pub(crate) fn metal_cache() -> Option<std::path::PathBuf> {
    let mut buffer = vec![0 as c_char; libc::PATH_MAX as usize];
    // SAFETY: writes at most `buffer.len()` bytes, NUL-terminated, into the buffer, and returns the size the value
    // needs (0 when there is none).
    let needed = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_CACHE_DIR,
            buffer.as_mut_ptr(),
            buffer.len(),
        )
    };
    if needed == 0 || needed > buffer.len() {
        return None;
    }
    // SAFETY: `confstr` wrote a NUL-terminated value that fits.
    let text = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().ok()?;
    std::fs::canonicalize(text).ok()
}

/// `sandbox_init_with_parameters`, if this macOS exports it.
fn with_parameters() -> Option<InitWithParameters> {
    // SAFETY: looks a symbol up in the images already loaded; nothing runs.
    let symbol =
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_init_with_parameters".as_ptr()) };
    // SAFETY: a non-null address of the function whose C signature `InitWithParameters` is.
    (!symbol.is_null())
        .then(|| unsafe { std::mem::transmute::<*mut c_void, InitWithParameters>(symbol) })
}

fn apply_with_parameters(
    init: InitWithParameters,
    parameters: &[(&str, String)],
) -> Result<(), String> {
    let text = c_text(PROFILE)?;
    let owned = parameters
        .iter()
        .flat_map(|(name, value)| [c_text(name), c_text(value)])
        .collect::<Result<Vec<_>, _>>()?;
    let mut list: Vec<*const c_char> = owned.iter().map(|s| s.as_ptr()).collect();
    list.push(std::ptr::null());
    let mut error = std::ptr::null_mut();
    // SAFETY: NUL-terminated text, a null-terminated list of NUL-terminated strings that outlive the call, and an
    // out-pointer for the error, which the call allocates.
    let status = unsafe { init(text.as_ptr(), 0, list.as_ptr(), &mut error) };
    checked(status, error)
}

fn apply_escaped(parameters: &[(&str, String)]) -> Result<(), String> {
    let text = c_text(&escaped_profile(parameters)?)?;
    let mut error = std::ptr::null_mut();
    // SAFETY: NUL-terminated text and an out-pointer for the error, which the call allocates.
    let status = unsafe { sandbox_init(text.as_ptr(), 0, &mut error) };
    checked(status, error)
}

/// A call's status, with the kernel's message (freed here) when it refused.
fn checked(status: c_int, error: *mut c_char) -> Result<(), String> {
    if error.is_null() {
        return if status == 0 {
            Ok(())
        } else {
            Err(format!("sandbox refused ({status})"))
        };
    }
    // SAFETY: the NUL-terminated message the call allocated, read once, then freed with its own function.
    let message = unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned();
    unsafe { sandbox_free_error(error) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("sandbox refused: {message}"))
    }
}

fn c_text(text: &str) -> Result<CString, String> {
    CString::new(text).map_err(|_| "a NUL in the profile".to_owned())
}
