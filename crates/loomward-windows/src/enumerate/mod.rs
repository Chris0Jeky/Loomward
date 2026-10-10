//! Validated metadata enumeration. Never opens file contents or follows reparse directories.
use std::path::Path;

/// Actual native enumeration class, including the width-preserving fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Extended,
    Both,
    Find,
    Portable,
}
/// A file ID as observed. The width is kept: a 64-bit ID is never padded to "128".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileIdObs {
    /// 128-bit `FILE_ID_128` (extended listing or `FileIdInfo`).
    Id128([u8; 16]),
    /// 64-bit ID from `FileIdBothDirectoryInfo`; not unique on ReFS (N3).
    Id64(u64),
}

impl FileIdObs {
    /// All-zero provider IDs carry no uniqueness evidence; retain name-only observation.
    pub fn nonzero(self) -> Option<Self> {
        match self {
            Self::Id64(0) => None,
            Self::Id128(id) if id == [0; 16] => None,
            id => Some(id),
        }
    }
}

/// One directory entry as listed, before any open. Never read from file content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry<'a> {
    /// Exact UTF-16 name; may hold unpaired surrogates.
    pub name: &'a [u16],
    /// The listed ID, `None` when the strategy reports none.
    pub file_id: Option<FileIdObs>,
    /// Win32 file attributes.
    pub attributes: u32,
    /// Reparse tag; `None` when the strategy cannot report it.
    pub reparse_tag: Option<u32>,
    /// Default-stream logical size as listed.
    pub end_of_file: u64,
    /// Default-stream allocation as listed; `None` when unknown.
    pub allocation_size: Option<u64>,
    /// Creation time in 100 ns ticks since 1601 UTC, if listed.
    pub creation: Option<i64>,
    /// Last-write time, if listed.
    pub last_write: Option<i64>,
    /// Change time, if listed.
    pub change: Option<i64>,
    /// Last-access time, if listed.
    pub last_access: Option<i64>,
}

/// What the sink tells the lister to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep listing.
    Continue,
    /// Stop (cancel or limit); the outcome is `Incomplete`.
    Stop,
}

/// Why a listing stopped before the end of the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteReason {
    /// The run was cancelled.
    Cancelled,
    /// Access denied part-way.
    AccessDenied,
    /// A native error code.
    Io(i32),
    /// A record failed buffer validation, or a zero-byte success could not grow the buffer.
    Malformed,
    /// Listing was observed outside a targeted run's reconciliation scope; upserts only.
    OutsideScope,
    /// A scan budget was hit.
    Limit,
}

/// How a listing ended. Only `Complete` may establish absence; every other stop is `Incomplete`
/// (upserts only, unseen children stay stale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListOutcome {
    /// End of directory (`ERROR_NO_MORE_FILES` or the portable equivalent).
    Complete,
    /// Any other stop.
    Incomplete(IncompleteReason),
}

/// Where an opened directory's identity came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdBasis {
    /// The listing's ID, confirmed by the post-open query.
    Listed,
    /// The listing had none; the post-open ID is the identity.
    PostOpen,
    /// Observed but non-unique ReFS 64-bit ID; name matching only.
    NonUnique,
    /// Neither: identity quality `path_observation`, no durable references.
    None,
}

/// The facts read from an opened (and post-open validated) directory handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenedIdentity {
    /// The identity, width preserved.
    pub id: Option<FileIdObs>,
    /// Where `id` came from.
    pub basis: IdBasis,
    /// Volume serial observed from the same handle.
    pub volume_serial: Option<u64>,
    /// Attributes read after the open.
    pub attributes: u32,
    /// Reparse tag read after the open.
    pub reparse_tag: Option<u32>,
}

/// Why a directory could not be opened or validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceError {
    /// Reparse point, offline or recall-on-access: refused, never listed or hydrated.
    Refused,
    /// The listed and post-open IDs disagree; the directory is recorded `partial`.
    IdentityChanged,
    /// The listing had an ID but the post-open query could not verify it.
    IdentityUnverified,
    /// Access denied.
    AccessDenied,
    /// A native error code.
    Io(i32),
    /// The strategy is not available on this platform or volume.
    Unsupported,
}

/// Read-only directory source; implementations validate each handle before listing.
pub trait DirSource: Send + Sync {
    /// Owned directory handle. Kept alive until synchronous calls return.
    type Dir: Send + Sync;
    /// Whether this validated directory is on ReFS; 64-bit IDs there are non-unique.
    fn is_refs(&self, _dir: &Self::Dir) -> bool {
        false
    }
    /// Preferred enumeration strategy (actual fallbacks are reported by the native directory).
    fn strategy(&self) -> Strategy;
    /// Actual class used by this opened directory, after any capability fallback.
    fn strategy_for(&self, _dir: &Self::Dir) -> Strategy {
        self.strategy()
    }
    /// Open an explicitly granted path, without following its final reparse component.
    fn open_root(&self, path: &Path) -> Result<(Self::Dir, OpenedIdentity), SourceError>;
    /// Enumerate metadata, returning Complete only on native end-of-directory.
    fn list(&self, dir: &Self::Dir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome;
    /// Open a single child name relative to its validated parent.
    fn open_child(
        &self,
        parent: &Self::Dir,
        entry: &RawEntry<'_>,
    ) -> Result<(Self::Dir, OpenedIdentity), SourceError>;
}

/// Attributes that may redirect or hydrate a directory.
pub fn refused_attributes(attributes: u32) -> bool {
    attributes & (0x400 | 0x1000 | 0x40000 | 0x400000) != 0
}

/// ID comparison keeps width; a missing verification never trusts a listed ID (#117 partial 2).
pub fn validate_identity(
    listed: Option<FileIdObs>,
    opened: Option<FileIdObs>,
    refs: bool,
) -> Result<(Option<FileIdObs>, IdBasis), SourceError> {
    let basis = match (listed, opened) {
        (Some(a), Some(b)) if a != b => return Err(SourceError::IdentityChanged),
        (Some(_), None) => return Err(SourceError::IdentityUnverified),
        (Some(_), Some(_)) => IdBasis::Listed,
        (None, Some(_)) => IdBasis::PostOpen,
        (None, None) => IdBasis::None,
    };
    Ok((
        opened,
        if refs && matches!(opened, Some(FileIdObs::Id64(_))) {
            IdBasis::NonUnique
        } else {
            basis
        },
    ))
}

/// Decode a successful buffer. A final record ends this buffer, never the directory.
pub fn decode_buffer(
    buffer: &[u8],
    strategy: Strategy,
    sink: &mut dyn FnMut(RawEntry<'_>) -> Flow,
) -> Result<(), IncompleteReason> {
    let header = match strategy {
        Strategy::Extended => 88,
        Strategy::Both => 104,
        _ => return Err(IncompleteReason::Malformed),
    };
    let malformed = IncompleteReason::Malformed;
    let mut offset = 0usize;
    loop {
        let b = buffer.get(offset..).ok_or(malformed)?;
        if b.len() < header {
            return Err(malformed);
        }
        let u32_at = |n| u32::from_le_bytes(b[n..n + 4].try_into().unwrap());
        let i64_at = |n| i64::from_le_bytes(b[n..n + 8].try_into().unwrap());
        let next = u32_at(0) as usize;
        if next != 0 && (next % 8 != 0 || next < header || next >= b.len()) {
            return Err(malformed);
        }
        let record_len = if next == 0 { b.len() } else { next };
        let name_len = u32_at(60) as usize;
        if name_len == 0 || name_len % 2 != 0 || name_len > record_len - header {
            return Err(malformed);
        }
        let name: Vec<u16> = b[header..header + name_len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if i64_at(40) < 0 || i64_at(48) < 0 || name.contains(&0) {
            return Err(malformed);
        }
        let entry = RawEntry {
            name: &name,
            file_id: (if strategy == Strategy::Extended {
                FileIdObs::Id128(b[72..88].try_into().unwrap())
            } else {
                FileIdObs::Id64(u64::from_le_bytes(b[96..104].try_into().unwrap()))
            })
            .nonzero(),
            attributes: u32_at(56),
            reparse_tag: if strategy == Strategy::Extended && u32_at(56) & 0x400 != 0 {
                Some(u32_at(68))
            } else {
                None
            },
            end_of_file: i64_at(40) as u64,
            allocation_size: Some(i64_at(48) as u64),
            creation: Some(i64_at(8)),
            last_access: Some(i64_at(16)),
            last_write: Some(i64_at(24)),
            change: Some(i64_at(32)),
        };
        if name != [46] && name != [46, 46] && sink(entry) == Flow::Stop {
            return Err(IncompleteReason::Cancelled);
        }
        if next == 0 {
            return Ok(());
        }
        offset = offset.checked_add(next).ok_or(malformed)?;
    }
}

/// Only ERROR_NO_MORE_FILES proves EOF; success without a record is malformed at the growth cap.
pub fn query_stop(code: u32, cancelled: bool) -> ListOutcome {
    if cancelled {
        return ListOutcome::Incomplete(IncompleteReason::Cancelled);
    }
    match code {
        18 => ListOutcome::Complete,
        5 => ListOutcome::Incomplete(IncompleteReason::AccessDenied),
        0 => ListOutcome::Incomplete(IncompleteReason::Malformed),
        c => ListOutcome::Incomplete(IncompleteReason::Io(c as i32)),
    }
}
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub(crate) use native::open_watch_root;
#[cfg(windows)]
pub use native::{running_elevated, IoCancellation, NativeDir, NativeSource};
#[cfg(test)]
mod tests;
