//! Recycle Bin size for one volume root (LW-131, docs/52 sections 4 and 6).
//!
//! One `SHQueryRecycleBinW` metadata query, made only when the given path is exactly a volume
//! root. This module never enumerates, opens, empties or modifies the bin; every other path,
//! an elevated process, a non-Windows host or any error is unknown (`None`), never zero.

use std::path::Path;

/// Windows-reported totals for one volume's Recycle Bin (logical sizes of the recycled items).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RecycleBinSize {
    /// Total size of the recycled items, as Windows reports it.
    pub bytes: u64,
    /// Number of recycled items.
    pub items: u64,
}

/// Unknown off Windows.
#[cfg(not(windows))]
pub fn query(_volume_root: &Path) -> Option<RecycleBinSize> {
    None
}

/// The bin's size when `volume_root` is exactly a volume root (`C:\` or a mounted-folder root);
/// any other path is refused before the query, because the bin is per volume (docs/52 section 6).
#[cfg(windows)]
pub fn query(volume_root: &Path) -> Option<RecycleBinSize> {
    use windows_sys::Win32::{
        Storage::FileSystem::GetVolumePathNameW,
        UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO},
    };
    crate::win::non_elevated().ok()?;
    let path = crate::win::wide(volume_root).ok()?;
    let mut mount = [0u16; 1024];
    if unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return None;
    }
    let length = mount.iter().position(|&c| c == 0)?;
    if !same_root(&path[..path.len() - 1], &mount[..length]) {
        return None;
    }
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..Default::default()
    };
    // `mount` is NUL-terminated at `length`.
    let hresult = unsafe { SHQueryRecycleBinW(mount.as_ptr(), &mut info) };
    size_from(hresult, info.i64Size, info.i64NumItems)
}

/// A failed `HRESULT` (negative) or a negative size or count is unknown, never zero or a clamp.
#[cfg(any(windows, test))]
fn size_from(hresult: i32, bytes: i64, items: i64) -> Option<RecycleBinSize> {
    if hresult < 0 {
        return None;
    }
    Some(RecycleBinSize {
        bytes: u64::try_from(bytes).ok()?,
        items: u64::try_from(items).ok()?,
    })
}

/// Equal up to trailing separators and ASCII case (drive letters and volume GUID paths).
#[cfg(any(windows, test))]
fn same_root(path: &[u16], mount: &[u16]) -> bool {
    const BACKSLASH: u16 = b'\\' as u16;
    const SLASH: u16 = b'/' as u16;
    let trim = |mut s: &[u16]| {
        while let [rest @ .., BACKSLASH | SLASH] = s {
            s = rest;
        }
        String::from_utf16_lossy(s).to_ascii_lowercase()
    };
    !mount.is_empty() && trim(path) == trim(mount)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn recycle_bin_root_match_ignores_trailing_separators_and_ascii_case_only() {
        assert!(same_root(&wide("C:\\"), &wide("C:\\")));
        assert!(same_root(&wide("c:"), &wide("C:\\")));
        assert!(same_root(&wide("D:\\mnt\\vol"), &wide("D:\\mnt\\vol\\")));
        assert!(!same_root(&wide("C:\\Users"), &wide("C:\\")));
        assert!(!same_root(&wide("C:\\x"), &wide("C:\\")));
        assert!(!same_root(&wide("C:\\"), &wide("")));
    }

    #[test]
    fn recycle_bin_size_is_unknown_for_a_folder_that_is_not_a_volume_root() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(query(temp.path()), None);
        assert_eq!(query(Path::new("synthetic\0name")), None);
    }

    #[test]
    fn recycle_bin_failed_hresult_and_negative_sizes_are_unknown_never_zero() {
        // E_FAIL and a generic failure code, even with plausible sizes beside them.
        assert_eq!(size_from(-2147467259, 10, 1), None);
        assert_eq!(size_from(-1, 0, 0), None);
        // Each out-of-range field alone makes the whole answer unknown.
        assert_eq!(size_from(0, -1, 0), None);
        assert_eq!(size_from(0, 0, -1), None);
        assert_eq!(size_from(0, i64::MIN, i64::MIN), None);
        // A genuinely empty bin is a known zero, and S_FALSE (positive) is still a success code.
        let empty = Some(RecycleBinSize { bytes: 0, items: 0 });
        assert_eq!(size_from(0, 0, 0), empty);
        assert_eq!(size_from(1, 0, 0), empty);
        assert_eq!(
            size_from(0, i64::MAX, 3),
            Some(RecycleBinSize {
                bytes: i64::MAX as u64,
                items: 3
            })
        );
    }

    /// An elevated process never reaches the shell query, even for a real volume root.
    #[cfg(windows)]
    #[test]
    fn recycle_bin_size_is_unknown_when_elevated() {
        if crate::win::non_elevated().is_ok() {
            return;
        }
        let root = std::env::var("SystemDrive").unwrap() + "\\";
        assert_eq!(query(Path::new(&root)), None);
    }

    /// Reads only the system volume's bin totals; run by hand to prove the FFI shape.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads the host system volume's Recycle Bin totals (metadata only)"]
    fn recycle_bin_size_reads_a_volume_root() {
        let root = std::env::var("SystemDrive").unwrap() + "\\";
        assert!(query(Path::new(&root)).is_some());
    }
}
