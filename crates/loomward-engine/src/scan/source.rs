//! The enumeration seam of docs/41 section 4.1: what the engine needs from a directory source.
//!
//! Owner: lane L7 (LW-006, LW-101). The Windows implementation lives in
//! `loomward-windows::enumerate`; a portable `std::fs::read_dir` source runs the tests on every
//! platform. If `loomward-windows` ships other names, the adapter goes in this file.
//!
//! #117 errata items: N3 (a 64-bit ID is never padded to 128 bits or treated as unique on ReFS)
//! and partial #2 (the post-open ID policy) both hinge on [`FileIdObs`] keeping its width and on
//! [`OpenedIdentity::basis`]. Types only here; the strategy chain is L7 work.

use super::GrantedRoot;
use loomward_protocol::EnumerationStrategy;

/// A file ID as observed. The width is kept: a 64-bit ID is never padded to "128".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileIdObs {
    /// 128-bit `FILE_ID_128` (extended listing or `FileIdInfo`).
    Id128([u8; 16]),
    /// 64-bit ID from `FileIdBothDirectoryInfo`; not unique on ReFS (N3).
    Id64(u64),
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
    /// Access denied.
    AccessDenied,
    /// A native error code.
    Io(i32),
    /// The strategy is not available on this platform or volume.
    Unsupported,
}

/// A directory source. Never opens a file and never reads content.
pub trait DirSource: Send + Sync {
    /// An open directory handle.
    type Dir: Send;

    /// The strategy this source uses on the granted volume.
    fn strategy(&self) -> EnumerationStrategy;

    /// Opens the granted root without following reparse points, validates it post-open and
    /// returns its identity.
    fn open_root(&self, root: &GrantedRoot) -> Result<(Self::Dir, OpenedIdentity), SourceError>;

    /// Streams every entry in validated buffer chunks into `sink`.
    fn list(&self, dir: &Self::Dir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome;

    /// Opens a child directory relative to its parent handle by exact UTF-16 name, then
    /// validates it post-open.
    fn open_child(
        &self,
        parent: &Self::Dir,
        entry: &RawEntry<'_>,
    ) -> Result<(Self::Dir, OpenedIdentity), SourceError>;
}
