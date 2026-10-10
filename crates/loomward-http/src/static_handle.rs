use std::fs::File;
use std::path::PathBuf;

#[cfg(windows)]
pub(super) fn path(file: &File) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    #[link(name = "Kernel32")]
    extern "system" {
        fn GetFinalPathNameByHandleW(
            handle: *mut std::ffi::c_void,
            path: *mut u16,
            len: u32,
            flags: u32,
        ) -> u32;
    }
    let mut path = vec![0u16; 32768];
    // SAFETY: the file handle is live and the output buffer covers the advertised length.
    let len = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            path.as_mut_ptr(),
            path.len() as u32,
            0,
        )
    } as usize;
    if len == 0 || len >= path.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&path[..len])))
}

#[cfg(target_os = "linux")]
pub(super) fn path(file: &File) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()
}

#[cfg(target_os = "macos")]
pub(super) fn path(file: &File) -> Option<PathBuf> {
    use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
    extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    let mut path = [0u8; 1024];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN bytes to the live output buffer.
    if unsafe { fcntl(file.as_raw_fd(), 50, path.as_mut_ptr()) } == -1 {
        return None;
    }
    let len = path.iter().position(|&b| b == 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&path[..len])))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(super) fn path(_: &File) -> Option<PathBuf> {
    None
}
