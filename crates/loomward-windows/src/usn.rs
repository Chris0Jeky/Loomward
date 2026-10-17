//! Bounded, non-elevated journal hints. Records carry identity, never authority or paths.
use crate::enumerate::SourceError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bookmark {
    pub journal_id: u64,
    pub next_usn: i64,
}

impl Bookmark {
    pub fn retained_by(self, current: Self, first_usn: i64) -> bool {
        self.journal_id == current.journal_id
            && self.next_usn >= first_usn
            && self.next_usn <= current.next_usn
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub file_id: Vec<u8>,
    pub parent_id: Vec<u8>,
    pub reason: u32,
    pub is_directory: bool,
}

/// Parse V2/V3 without interpreting redacted or untrusted names. Preserve identity widths.
pub fn decode(bytes: &[u8]) -> Result<(i64, Vec<Record>), SourceError> {
    let bad = SourceError::Refused;
    let next = i64::from_le_bytes(bytes.get(..8).ok_or(bad)?.try_into().unwrap());
    let mut records = Vec::new();
    let mut at = 8;
    while at < bytes.len() {
        let header = bytes.get(at..at + 8).ok_or(bad)?;
        let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let version = u16::from_le_bytes(header[4..6].try_into().unwrap());
        let (minimum, width, reason_at, name_at) = match version {
            2 => (60, 8, 40, 56),
            3 => (76, 16, 56, 72),
            _ => return Err(bad),
        };
        if len < minimum || len % 8 != 0 {
            return Err(bad);
        }
        let end = at.checked_add(len).ok_or(bad)?;
        let record = bytes.get(at..end).ok_or(bad)?;
        let name_len =
            u16::from_le_bytes(record[name_at..name_at + 2].try_into().unwrap()) as usize;
        let name_start =
            u16::from_le_bytes(record[name_at + 2..name_at + 4].try_into().unwrap()) as usize;
        if name_len % 2 != 0
            || name_start + name_len > len
            || (name_len != 0 && (name_start < minimum || name_start % 2 != 0))
        {
            return Err(bad);
        }
        records.push(Record {
            file_id: record[8..8 + width].to_vec(),
            parent_id: record[8 + width..8 + 2 * width].to_vec(),
            reason: u32::from_le_bytes(record[reason_at..reason_at + 4].try_into().unwrap()),
            is_directory: u32::from_le_bytes(
                record[reason_at + 12..reason_at + 16].try_into().unwrap(),
            ) & 0x10
                != 0,
        });
        at = end;
    }
    Ok((next, records))
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::enumerate::{open_journal_root, OpenedIdentity};
    use std::{
        fs::File,
        mem::{size_of, zeroed},
        os::windows::io::AsRawHandle,
        path::Path,
        ptr::null_mut,
    };
    use windows_sys::Win32::{
        Foundation::GetLastError,
        System::{Ioctl::*, IO::DeviceIoControl},
    };

    pub struct Journal {
        _pins: Vec<File>,
        file: File,
    }
    impl Journal {
        pub fn open(path: &Path, expected: OpenedIdentity) -> Result<Self, SourceError> {
            let (pins, file, actual) = open_journal_root(path)?;
            if actual.id != expected.id || actual.volume_serial != expected.volume_serial {
                return Err(SourceError::IdentityChanged);
            }
            Ok(Self { _pins: pins, file })
        }
        pub fn head(&self) -> Result<(Bookmark, i64), SourceError> {
            let mut data: USN_JOURNAL_DATA_V1 = unsafe { zeroed() };
            let mut returned = 0;
            if unsafe {
                DeviceIoControl(
                    self.file.as_raw_handle(),
                    FSCTL_QUERY_USN_JOURNAL,
                    null_mut(),
                    0,
                    (&mut data as *mut USN_JOURNAL_DATA_V1).cast(),
                    size_of::<USN_JOURNAL_DATA_V1>() as u32,
                    &mut returned,
                    null_mut(),
                )
            } == 0
            {
                return Err(SourceError::Io(unsafe { GetLastError() } as i32));
            }
            if returned < 56 {
                return Err(SourceError::Refused);
            }
            Ok((
                Bookmark {
                    journal_id: data.UsnJournalID,
                    next_usn: data.NextUsn,
                },
                data.FirstUsn.max(data.LowestValidUsn),
            ))
        }
        /// Read to the captured head, at most 16 MiB. Exhaustion and gaps require a full refresh.
        /// The callback must filter by granted-root ancestry before retaining a record.
        pub fn catch_up(
            &self,
            bookmark: Bookmark,
            mut visit: impl FnMut(Record),
        ) -> Result<Bookmark, SourceError> {
            let (head, first) = self.head()?;
            if !bookmark.retained_by(head, first) {
                return Err(SourceError::Refused);
            }
            let mut cursor = bookmark.next_usn;
            let mut buffer = vec![0u64; 8192];
            for _ in 0..256 {
                if cursor >= head.next_usn {
                    return Ok(head);
                }
                let input = READ_USN_JOURNAL_DATA_V1 {
                    StartUsn: cursor,
                    ReasonMask: u32::MAX,
                    ReturnOnlyOnClose: 0,
                    Timeout: 0,
                    BytesToWaitFor: 0,
                    UsnJournalID: head.journal_id,
                    MinMajorVersion: 2,
                    MaxMajorVersion: 3,
                };
                let mut returned = 0;
                if unsafe {
                    DeviceIoControl(
                        self.file.as_raw_handle(),
                        FSCTL_READ_UNPRIVILEGED_USN_JOURNAL,
                        (&input as *const READ_USN_JOURNAL_DATA_V1).cast(),
                        size_of::<READ_USN_JOURNAL_DATA_V1>() as u32,
                        buffer.as_mut_ptr().cast(),
                        (buffer.len() * 8) as u32,
                        &mut returned,
                        null_mut(),
                    )
                } == 0
                {
                    return Err(SourceError::Io(unsafe { GetLastError() } as i32));
                }
                if returned as usize > buffer.len() * 8 {
                    return Err(SourceError::Refused);
                }
                let bytes = unsafe {
                    std::slice::from_raw_parts(buffer.as_ptr().cast(), returned as usize)
                };
                let (next, records) = decode(bytes)?;
                if next <= cursor {
                    return Err(SourceError::Refused);
                }
                for record in records {
                    visit(record);
                }
                cursor = next;
            }
            Err(SourceError::Refused)
        }
    }
}
#[cfg(windows)]
pub use native::Journal;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoder_keeps_v2_width_and_directory_bit() {
        let mut bytes = vec![0; 72];
        bytes[8..12].copy_from_slice(&64u32.to_le_bytes());
        bytes[12..14].copy_from_slice(&2u16.to_le_bytes());
        bytes[16..24].copy_from_slice(&5u64.to_le_bytes());
        bytes[24..32].copy_from_slice(&6u64.to_le_bytes());
        bytes[60..64].copy_from_slice(&0x10u32.to_le_bytes());
        let (_, records) = decode(&bytes).unwrap();
        assert_eq!(records[0].file_id, 5u64.to_le_bytes());
        assert_eq!(records[0].parent_id, 6u64.to_le_bytes());
        assert!(records[0].is_directory);
    }
    #[test]
    fn wrap_recreation_and_future_bookmarks_require_reconciliation() {
        let head = Bookmark {
            journal_id: 4,
            next_usn: 100,
        };
        assert!(Bookmark {
            next_usn: 50,
            ..head
        }
        .retained_by(head, 50));
        for old in [
            Bookmark {
                journal_id: 3,
                ..head
            },
            Bookmark {
                next_usn: 49,
                ..head
            },
            Bookmark {
                next_usn: 101,
                ..head
            },
        ] {
            assert!(!old.retained_by(head, 50));
        }
    }
    #[test]
    fn decoder_keeps_v3_identity_and_refuses_bad_records() {
        let mut bytes = vec![0; 88];
        bytes[..8].copy_from_slice(&100i64.to_le_bytes());
        bytes[8..12].copy_from_slice(&80u32.to_le_bytes());
        bytes[12..14].copy_from_slice(&3u16.to_le_bytes());
        bytes[16..32].fill(1);
        bytes[32..48].fill(2);
        let (next, records) = decode(&bytes).unwrap();
        assert_eq!(next, 100);
        assert_eq!(records[0].file_id, vec![1; 16]);
        assert_eq!(records[0].parent_id, vec![2; 16]);
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_err() || end == 8);
        }
        bytes[80..82].copy_from_slice(&2u16.to_le_bytes());
        assert!(decode(&bytes).is_err());
        bytes[12..14].copy_from_slice(&4u16.to_le_bytes());
        assert!(decode(&bytes).is_err());
    }
}
