use serde::{Deserialize, Serialize};
use std::{io, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentityQuality {
    FileId128,
    Legacy64 { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectKey {
    /// Decimal text keeps the native serial exact in JSON consumers.
    pub volume_serial: String,
    pub file_id: [u8; 16],
    pub quality: IdentityQuality,
    pub creation_time: String,
    pub reparse_tag: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedIdentity {
    #[serde(flatten)]
    pub object: ObjectKey,
    pub link_count: u32,
    pub attributes: u32,
    /// Decimal byte quantities; timestamps are Windows 100ns ticks since 1601 UTC.
    pub size: String,
    pub allocation: String,
    pub last_access_time: String,
    pub last_write_time: String,
    pub change_time: String,
    /// Root-to-parent bindings, including reparse objects, without disclosing names.
    pub ancestors: Vec<ObjectKey>,
}

impl ObservedIdentity {
    pub fn same_object(&self, other: &Self) -> bool {
        self.object == other.object
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    Same,
    Changed,
    Recreated,
    Gone,
    Unsupported,
}

/// A point-in-time metadata comparison, not race-free authority or permission.
pub fn verify_current(path: &Path, expected: &ObservedIdentity) -> Verification {
    match observe(path) {
        Ok(current)
            if current.ancestors != expected.ancestors || !current.same_object(expected) =>
        {
            Verification::Recreated
        }
        Ok(current) if current != *expected => Verification::Changed,
        Ok(_) => Verification::Same,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Verification::Gone,
        Err(_) => Verification::Unsupported,
    }
}

#[cfg(not(windows))]
pub fn observe(_: &Path) -> io::Result<ObservedIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows handle identity is unavailable on this platform",
    ))
}

#[cfg(windows)]
pub fn observe(path: &Path) -> io::Result<ObservedIdentity> {
    use std::path::Component;
    // Preserve reparse entries: canonicalize would silently follow the final link.
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "parent traversal is not an observation binding",
        ));
    }
    let path = std::path::absolute(path)?;
    let ancestors = ancestry(&path)?;
    let mut result = one(&path)?;
    if ancestors != ancestry(&path)? {
        return Err(io::Error::other(
            "ancestor binding changed during observation",
        ));
    }
    result.ancestors = ancestors;
    Ok(result)
}

#[cfg(windows)]
fn ancestry(path: &Path) -> io::Result<Vec<ObjectKey>> {
    let mut paths: Vec<_> = path
        .ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    paths.reverse();
    paths
        .into_iter()
        .map(|p| one(p).map(|o| o.object))
        .collect()
}

#[cfg(windows)]
fn one(path: &Path) -> io::Result<ObservedIdentity> {
    use crate::win::{info, open};
    use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};
    let handle = open(
        path,
        FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )?;
    let basic: FILE_BASIC_INFO = info(&handle, FileBasicInfo)?;
    let standard: FILE_STANDARD_INFO = info(&handle, FileStandardInfo)?;
    let tag: FILE_ATTRIBUTE_TAG_INFO = info(&handle, FileAttributeTagInfo)?;
    let (serial, id, quality) = match info::<FILE_ID_INFO>(&handle, FileIdInfo) {
        Ok(id) => (
            id.VolumeSerialNumber,
            id.FileId.Identifier,
            IdentityQuality::FileId128,
        ),
        Err(error) if matches!(error.raw_os_error(), Some(code) if [ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_INVALID_FUNCTION, ERROR_CALL_NOT_IMPLEMENTED].contains(&(code as u32))) =>
        {
            let mut legacy = BY_HANDLE_FILE_INFORMATION::default();
            if unsafe { GetFileInformationByHandle(handle.0, &mut legacy) } == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!(
                        "FileIdInfo: {error}; legacy identity: {}",
                        io::Error::last_os_error()
                    ),
                ));
            }
            let mut id = [0; 16];
            id[..8].copy_from_slice(
                &((u64::from(legacy.nFileIndexHigh) << 32) | u64::from(legacy.nFileIndexLow))
                    .to_le_bytes(),
            );
            (
                u64::from(legacy.dwVolumeSerialNumber),
                id,
                IdentityQuality::Legacy64 {
                    reason: error.to_string(),
                },
            )
        }
        Err(error) => return Err(error),
    };
    if id == [0; 16] {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem returned no usable file identifier",
        ));
    }
    if standard.EndOfFile < 0 || standard.AllocationSize < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "negative native size",
        ));
    }
    Ok(ObservedIdentity {
        object: ObjectKey {
            volume_serial: serial.to_string(),
            file_id: id,
            quality,
            creation_time: basic.CreationTime.to_string(),
            reparse_tag: tag.ReparseTag,
        },
        link_count: standard.NumberOfLinks,
        attributes: tag.FileAttributes,
        size: standard.EndOfFile.to_string(),
        allocation: standard.AllocationSize.to_string(),
        last_access_time: basic.LastAccessTime.to_string(),
        last_write_time: basic.LastWriteTime.to_string(),
        change_time: basic.ChangeTime.to_string(),
        ancestors: Vec::new(),
    })
}
