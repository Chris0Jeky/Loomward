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

#[cfg(test)]
mod tests {
    use super::path;

    /// #224: the handle's own path is what the static root check reads, on every platform.
    #[test]
    fn a_handle_reports_its_canonical_path() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let handle = std::fs::File::open(&file).unwrap();
        assert_eq!(path(&handle), Some(file.canonicalize().unwrap()));
    }

    /// #224: a link opened from inside the root reports the target outside it, so the root check
    /// fails closed. Not compiled or run on the Windows development host (Linux and macOS legs).
    #[cfg(not(windows))]
    #[test]
    fn a_symlink_out_of_the_root_reports_the_target_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside.txt");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&outside, b"x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
        let handle = std::fs::File::open(root.join("link.txt")).unwrap();
        let reported = path(&handle).unwrap();
        assert_eq!(reported, outside.canonicalize().unwrap());
        assert!(!reported.starts_with(root.canonicalize().unwrap()));
    }
}
