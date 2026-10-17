//! LW-055: the service retains this directory across the owner's replace confirmation.
//! Child checks, temporary creation, publication and cleanup never resolve its pathname again.
//! Selection, the preview digest and replacement consent remain the diagnostic service's gates.
use crate::enumerate;
use std::{
    ffi::OsStr,
    fs::{File, Metadata},
    io::{self, Write},
    mem::{offset_of, size_of},
    os::windows::{ffi::OsStrExt, io::AsRawHandle, io::FromRawHandle},
    path::Path,
    ptr,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FileRenameInformation, NtCreateFile, NtSetInformationFile, FILE_CREATE,
            FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{Foundation::*, Storage::FileSystem::*, System::IO::IO_STATUS_BLOCK},
};

fn nt_error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(code) } as i32)
}

fn source_error(error: enumerate::SourceError) -> io::Error {
    match error {
        enumerate::SourceError::Io(code) if code < 0 => nt_error(code),
        enumerate::SourceError::Io(code) => io::Error::from_raw_os_error(code),
        enumerate::SourceError::AccessDenied => io::ErrorKind::PermissionDenied.into(),
        _ => io::ErrorKind::InvalidInput.into(),
    }
}

fn component(name: &OsStr) -> io::Result<Vec<u16>> {
    let name: Vec<_> = name.encode_wide().collect();
    if name.is_empty()
        || name.len() > 255
        || name == [46]
        || name == [46, 46]
        || name.iter().any(|c| matches!(*c, 0 | 47 | 58 | 92))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(name)
}

#[test]
fn component_names_are_single_children() {
    for name in [
        "",
        ".",
        "..",
        "../report.json",
        r"a\report.json",
        "report.json:stream",
        "nul\0name",
    ] {
        assert!(component(OsStr::new(name)).is_err(), "{name:?}");
    }
    assert!(component(OsStr::new(&"a".repeat(256))).is_err());
    assert!(component(OsStr::new("report.json")).is_ok());
}

/// A local, plain directory opened without traversing reparses in its canonical path.
#[derive(Debug)]
pub struct ExportFolder(File);

impl ExportFolder {
    pub fn open(canonical: &Path) -> io::Result<Self> {
        let mut pins = enumerate::root_handles(
            canonical,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        )
        .map_err(source_error)?;
        Ok(Self(pins.pop().ok_or(io::ErrorKind::InvalidInput)?))
    }

    /// Metadata only, without following a final reparse or recalling file contents.
    pub fn metadata(&self, name: &OsStr) -> io::Result<Metadata> {
        enumerate::open_relative_mode(
            &self.0,
            &component(name)?,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            false,
        )
        .map_err(source_error)?
        .metadata()
    }

    /// Publish the held preview bytes; `replace` is the service's owner-confirmed decision.
    pub fn write_export(
        &self,
        name: &OsStr,
        temporary: &OsStr,
        bytes: &[u8],
        replace: bool,
    ) -> io::Result<()> {
        let name = component(name)?;
        let mut temporary = component(temporary)?;
        let mut unicode = UNICODE_STRING {
            Length: (temporary.len() * 2) as u16,
            MaximumLength: (temporary.len() * 2) as u16,
            Buffer: temporary.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: self.0.as_raw_handle(),
            ObjectName: &mut unicode,
            ..Default::default()
        };
        let mut status = IO_STATUS_BLOCK::default();
        let mut handle = ptr::null_mut();
        let code = unsafe {
            NtCreateFile(
                &mut handle,
                FILE_GENERIC_WRITE | DELETE,
                &attributes,
                &mut status,
                ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                0,
                FILE_CREATE,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                ptr::null(),
                0,
            )
        };
        if code < 0 {
            return Err(nt_error(code));
        }
        // Exclusive access keeps the temporary's bytes and identity ours through publication.
        let mut file = unsafe { File::from_raw_handle(handle) };
        let written = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            let length = size_of::<FILE_RENAME_INFO>()
                .max(offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2);
            let mut storage = vec![0u64; length.div_ceil(8)];
            let rename = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
            unsafe {
                (*rename).Anonymous.ReplaceIfExists = replace;
                (*rename).RootDirectory = self.0.as_raw_handle();
                (*rename).FileNameLength = (name.len() * 2) as u32;
                ptr::copy_nonoverlapping(
                    name.as_ptr(),
                    ptr::addr_of_mut!((*rename).FileName).cast::<u16>(),
                    name.len(),
                );
                let code = NtSetInformationFile(
                    file.as_raw_handle(),
                    &mut status,
                    rename.cast(),
                    length as u32,
                    FileRenameInformation,
                );
                if code < 0 {
                    return Err(nt_error(code));
                }
            }
            Ok(())
        })();
        if written.is_err() {
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            // Delete only the temporary we still hold, even if the parent pathname was swapped.
            unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileDispositionInfo,
                    ptr::from_ref(&disposition).cast(),
                    size_of::<FILE_DISPOSITION_INFO>() as u32,
                );
            }
        }
        written
    }
}
