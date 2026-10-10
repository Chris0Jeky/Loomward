//! Vocabulary shared across areas. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Logical,
    Allocated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageState {
    Complete,
    Partial,
    Stale,
    Unscanned,
    Denied,
    Excluded,
    Cancelled,
    Unknown,
}

/// Default pending the LW-101 spike: file_id_extd_directory_info (ADR-V3-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnumerationStrategy {
    FileIdExtdDirectoryInfo,
    NtQueryDirectoryFileEx,
    FindFirstFileExLargeFetch,
    StdReadDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtFamily {
    Document,
    Spreadsheet,
    Presentation,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Data,
    Model,
    Executable,
    System,
    Font,
    Other,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAttribute {
    Readonly,
    Hidden,
    System,
    Archive,
    Compressed,
    Sparse,
    Encrypted,
    Offline,
    NotContentIndexed,
    ReparsePoint,
    RecallOnOpen,
    RecallOnDataAccess,
    Pinned,
    Unpinned,
    Temporary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryFlag {
    SensitiveName,
    ProtectedContext,
    HardlinkSuspected,
    CloudPlaceholder,
    ReparseNotFollowed,
    ExcludedStateDir,
    AccessDenied,
    EnumerationError,
    IdentityUnavailable,
    NameLossy,
    OnDiskOnlyListing,
    ListingIncomplete,
}

/// Display only. Never accepted back as input; no command takes a path string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPath {
    pub text: Text<1024>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}
