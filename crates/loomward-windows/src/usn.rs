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
    use crate::enumerate::{
        expected_identity_matches, open_journal_root, volume_device, OpenedIdentity,
    };
    use std::{
        cell::OnceCell,
        fs::File,
        mem::{size_of, zeroed},
        os::windows::io::{AsRawHandle, FromRawHandle},
        path::Path,
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{GetLastError, INVALID_HANDLE_VALUE},
        Storage::FileSystem::*,
        System::{Ioctl::*, IO::DeviceIoControl},
    };

    pub struct Journal {
        _pins: Vec<File>,
        file: File,
        volume: OnceCell<Option<File>>,
    }
    impl Journal {
        pub fn open(path: &Path, expected: OpenedIdentity) -> Result<Self, SourceError> {
            let (pins, file, actual) = open_journal_root(path)?;
            expected_identity_matches(actual, expected)?;
            Ok(Self {
                _pins: pins,
                file,
                volume: OnceCell::new(),
            })
        }
        /// The current parent identity of `id` on the journal's volume, from the object's own USN
        /// metadata. A zero-access open by ID never follows reparse points or recalls cloud
        /// content, and the name in the returned record is never read. `None` is "unknown".
        pub fn parent_of(&self, id: &[u8]) -> Option<Vec<u8>> {
            const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
            // A directory hint is refused on this host; a zero-access volume handle is not raw IO.
            let volume = self
                .volume
                .get_or_init(|| {
                    let path = volume_device(&self.file).ok()?;
                    let handle = unsafe {
                        CreateFileW(
                            path.as_ptr(),
                            0,
                            SHARE,
                            null(),
                            OPEN_EXISTING,
                            0,
                            null_mut(),
                        )
                    };
                    (handle != INVALID_HANDLE_VALUE)
                        .then(|| unsafe { File::from_raw_handle(handle) })
                })
                .as_ref()?;
            let descriptor = match id.len() {
                8 => FILE_ID_DESCRIPTOR_0 {
                    FileId: i64::from_le_bytes(id.try_into().ok()?),
                },
                16 if id[8..] == [0; 8] => FILE_ID_DESCRIPTOR_0 {
                    FileId: i64::from_le_bytes(id[..8].try_into().ok()?),
                },
                16 => FILE_ID_DESCRIPTOR_0 {
                    ExtendedFileId: FILE_ID_128 {
                        Identifier: id.try_into().ok()?,
                    },
                },
                _ => return None,
            };
            let wide = id.len() == 16 && id[8..] != [0; 8];
            let descriptor = FILE_ID_DESCRIPTOR {
                dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
                Type: if wide { ExtendedFileIdType } else { FileIdType },
                Anonymous: descriptor,
            };
            let handle = unsafe {
                OpenFileById(
                    volume.as_raw_handle(),
                    &descriptor,
                    0,
                    SHARE,
                    null(),
                    crate::win::metadata_open_flags(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return None;
            }
            let object = unsafe { File::from_raw_handle(handle) };
            let input = READ_FILE_USN_DATA {
                MinMajorVersion: 2,
                MaxMajorVersion: 3,
            };
            // Header plus a 255-unit name fits in 1 KiB.
            let mut buffer = [0u64; 128];
            let mut returned = 0;
            if unsafe {
                DeviceIoControl(
                    object.as_raw_handle(),
                    FSCTL_READ_FILE_USN_DATA,
                    (&input as *const READ_FILE_USN_DATA).cast(),
                    size_of::<READ_FILE_USN_DATA>() as u32,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                    &mut returned,
                    null_mut(),
                )
            } == 0
            {
                return None;
            }
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast(), returned as usize) };
            parent_from_record(bytes, id)
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

/// The parent in one FSCTL_READ_FILE_USN_DATA record, only when the record is for `id`.
#[cfg_attr(not(windows), allow(dead_code))]
fn parent_from_record(bytes: &[u8], id: &[u8]) -> Option<Vec<u8>> {
    let width = match u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?) {
        2 => 8,
        3 => 16,
        _ => return None,
    };
    let wide = |id: &[u8]| {
        let mut id = id.to_vec();
        if id.len() == 8 {
            id.resize(16, 0);
        }
        id
    };
    let parent = bytes.get(8 + width..8 + 2 * width)?;
    (id.len() >= 8 && wide(&bytes[8..8 + width]) == wide(id)).then(|| parent.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoder_alignment_offsets_lengths_and_unknown_versions_never_panic() {
        for (version, minimum, name_at) in [(2u16, 60usize, 56usize), (3, 76, 72)] {
            let len = (minimum + 2).next_multiple_of(8);
            let mut valid = vec![0; 8 + len];
            valid[8..12].copy_from_slice(&(len as u32).to_le_bytes());
            valid[12..14].copy_from_slice(&version.to_le_bytes());
            valid[8 + name_at..10 + name_at].copy_from_slice(&2u16.to_le_bytes());
            valid[10 + name_at..12 + name_at].copy_from_slice(&(minimum as u16).to_le_bytes());
            // Padding belongs to the record and is not another header, even when nonzero.
            valid[8 + minimum + 2..].fill(0xff);
            assert_eq!(decode(&valid).unwrap().1.len(), 1);
            let mut malformed = Vec::new();
            let mut unaligned = valid.clone();
            unaligned[8..12].copy_from_slice(&((len - 1) as u32).to_le_bytes());
            malformed.push(unaligned);
            let mut odd_offset = valid.clone();
            odd_offset[10 + name_at..12 + name_at]
                .copy_from_slice(&((minimum + 1) as u16).to_le_bytes());
            malformed.push(odd_offset);
            let mut overflow = valid.clone();
            overflow[8 + name_at..10 + name_at].copy_from_slice(&65534u16.to_le_bytes());
            malformed.push(overflow);
            let mut unknown = valid;
            unknown[12..14].copy_from_slice(&99u16.to_le_bytes());
            malformed.push(unknown);
            for bytes in malformed {
                assert_eq!(
                    std::panic::catch_unwind(|| decode(&bytes)).unwrap(),
                    Err(SourceError::Refused)
                );
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn parent_of_walks_current_ancestry_to_the_self_parented_volume_root() {
        use std::os::windows::fs::OpenOptionsExt;
        if crate::enumerate::running_elevated() {
            return;
        }
        let base = std::env::current_dir()
            .unwrap()
            .join("target/test-fixtures");
        std::fs::create_dir_all(&base).unwrap();
        let temp = tempfile::tempdir_in(base).unwrap();
        let child = temp.path().join("synthetic-a").join("synthetic-b");
        std::fs::create_dir_all(&child).unwrap();
        let id = |path: &std::path::Path| {
            let file = std::fs::OpenOptions::new()
                .access_mode(0)
                .custom_flags(crate::win::metadata_open_flags())
                .open(path)
                .unwrap();
            crate::identity::from_file(&file)
                .unwrap()
                .object
                .file_id
                .to_vec()
        };
        let wide = |mut id: Vec<u8>| {
            if id.len() == 8 {
                id.resize(16, 0);
            }
            id
        };
        let (_, _, opened) = crate::enumerate::open_journal_root(temp.path()).unwrap();
        let journal = Journal::open(temp.path(), opened).unwrap();
        let mut node = id(&child);
        let mut chain = vec![node.clone()];
        for _ in 0..64 {
            let parent = wide(journal.parent_of(&node).expect("parent by id"));
            if parent == node {
                break;
            }
            chain.push(parent.clone());
            node = parent;
        }
        assert_eq!(chain[1], id(child.parent().unwrap()));
        assert!(chain.contains(&id(temp.path())), "ancestry passes the base");
        assert_eq!(
            wide(journal.parent_of(&node).unwrap()),
            node,
            "volume root is its own parent"
        );
        assert_eq!(journal.parent_of(&[0xff; 16]), None);
    }
    #[test]
    fn file_usn_data_parent_requires_the_requested_identity() {
        let mut v2 = vec![0; 64];
        v2[4..6].copy_from_slice(&2u16.to_le_bytes());
        v2[8..16].copy_from_slice(&5u64.to_le_bytes());
        v2[16..24].copy_from_slice(&6u64.to_le_bytes());
        let mut wide5 = 5u64.to_le_bytes().to_vec();
        wide5.resize(16, 0);
        for id in [&5u64.to_le_bytes()[..], &wide5] {
            assert_eq!(
                parent_from_record(&v2, id),
                Some(6u64.to_le_bytes().to_vec())
            );
        }
        assert_eq!(parent_from_record(&v2, &7u64.to_le_bytes()), None);
        assert_eq!(parent_from_record(&v2, &[]), None);
        assert_eq!(parent_from_record(&v2[..20], &5u64.to_le_bytes()), None);
        let mut v3 = vec![0; 80];
        v3[4..6].copy_from_slice(&3u16.to_le_bytes());
        v3[8..24].fill(1);
        v3[24..40].fill(2);
        assert_eq!(parent_from_record(&v3, &[1; 16]), Some(vec![2; 16]));
        v3[4..6].copy_from_slice(&4u16.to_le_bytes());
        assert_eq!(parent_from_record(&v3, &[1; 16]), None);
    }
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
