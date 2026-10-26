//! Sector geometry and aligned hashing shared by copy read-back and C1 tree verification.
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle},
    ptr,
};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};

const BLOCK: usize = 1024 * 1024;

/// Geometry follows the pinned object's volume, including mounted folders and subst aliases.
pub(crate) fn sector_for_file(file: &File) -> io::Result<usize> {
    let path = crate::enumerate::volume_device(file)
        .map_err(|_| io::Error::other("volume unavailable"))?;
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let volume = unsafe { File::from_raw_handle(handle) };
    physical_sector(&volume)
}

pub(crate) struct AlignedBuffer {
    storage: Vec<u8>,
    offset: usize,
}
impl AlignedBuffer {
    pub(crate) fn new(sector: usize) -> Self {
        let storage = vec![0; BLOCK + sector];
        let offset = storage.as_ptr().align_offset(sector);
        Self { storage, offset }
    }
    pub(crate) fn bytes(&mut self) -> &mut [u8] {
        &mut self.storage[self.offset..self.offset + BLOCK]
    }
}

pub(crate) fn physical_sector(volume: &File) -> io::Result<usize> {
    use windows_sys::Win32::System::{Ioctl::*, IO::DeviceIoControl};
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageAccessAlignmentProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    let mut alignment = STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR::default();
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            volume.as_raw_handle(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            ptr::from_ref(&query).cast(),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            ptr::from_mut(&mut alignment).cast(),
            size_of::<STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if returned < size_of::<STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR>() as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "truncated sector geometry",
        ));
    }
    sector_alignment(
        alignment.BytesPerLogicalSector,
        alignment.BytesPerPhysicalSector,
    )
}
pub(crate) fn sector_alignment(logical: u32, physical: u32) -> io::Result<usize> {
    if !logical.is_power_of_two()
        || !physical.is_power_of_two()
        || physical < logical
        || physical as usize > BLOCK
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown sector alignment",
        ));
    }
    Ok(physical as usize)
}
/// Hash exactly `size` bytes using sector-multiple requests. Short reads must end on a sector
/// boundary unless they finish the stream. The callback retains each caller's I/O and budget rules.
pub(crate) fn sector_hash<E>(
    size: u64,
    sector: usize,
    buffer: &mut [u8],
    mut read: impl FnMut(u64, &mut [u8], usize) -> Result<usize, E>,
    changed: impl Fn() -> E,
) -> Result<[u8; 32], E> {
    let mut offset = 0;
    let mut digest = Sha256::new();
    while offset < size {
        let logical = (size - offset).min(buffer.len() as u64) as usize;
        let request = logical.div_ceil(sector) * sector;
        let mut got = 0;
        while got < logical {
            let count = read(
                offset + got as u64,
                &mut buffer[got..request],
                logical - got,
            )?;
            got += count;
            if count == 0 || got > logical || (got < logical && got % sector != 0) {
                return Err(changed());
            }
        }
        digest.update(&buffer[..got]);
        offset += got as u64;
    }
    Ok(digest.finalize().into())
}
