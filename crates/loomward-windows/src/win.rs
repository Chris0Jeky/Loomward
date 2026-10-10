use std::{io, mem::size_of, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};

pub(crate) struct Handle(pub HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(crate) fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut text: Vec<_> = path.as_os_str().encode_wide().collect();
    if text.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"));
    }
    text.push(0);
    Ok(text)
}

pub(crate) fn metadata_open_flags() -> u32 {
    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL
}

pub(crate) fn open(path: &Path, access: u32, share: u32) -> io::Result<Handle> {
    let text = wide(path)?;
    let handle = unsafe {
        CreateFileW(
            text.as_ptr(),
            access,
            share,
            ptr::null(),
            OPEN_EXISTING,
            metadata_open_flags(),
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(Handle(handle))
    }
}

pub(crate) fn info<T: Default>(handle: &Handle, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
    let mut value = T::default();
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            class,
            (&mut value as *mut T).cast(),
            size_of::<T>() as u32,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}
