//! Jobs. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

/// placement.simulate is synchronous and bounded, never a job (semantics.md section 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Scan,
    Refit,
    Teacher,
    Duplicates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    CancelRequested,
    Cancelled,
    Failed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanBudget {
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_entries: Option<Int<1, 50000000>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_dirs: Option<Int<1, 8000000>>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub threads: Option<Int<1, 32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanStartRequestMode {
    Full,
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanStartRequest {
    pub root_id: RootId,
    pub mode: ScanStartRequestMode,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub budget: Option<ScanBudget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanProgressPhase {
    Queued,
    Enumerating,
    Committing,
    RollingUp,
    Sweeping,
    Finalising,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanLimitReason {
    /// Directory nesting reached the catalogue's chain limit (512 levels).
    DepthLimit,
    /// The directory arena (directories in flight, with their handles and held child names) reached its budget.
    DirectoryLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanProgress {
    pub phase: ScanProgressPhase,
    pub strategy: EnumerationStrategy,
    pub examined: Count,
    pub indexed_files: Count,
    /// Directories whose listing was read and indexed. A directory refused on open is catalogued
    /// by name (flagged access denied) but is not indexed, so it is not counted here.
    pub indexed_dirs: Count,
    pub skipped: Count,
    pub failed: Count,
    pub logical_bytes: Bytes,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub allocated_bytes: Option<Bytes>,
    /// Queued and active tasks at sampling time, possibly including the listing just finished.
    pub pending_dirs: Count,
    /// Waiting commands, including sender backpressure and deferred commands; excludes processing.
    pub writer_queue_depth: Count,
    pub entries_per_second: Rate,
    pub elapsed_ms: Count,
    pub limit_hit: bool,
    /// Present only when a named cap stopped the scan (`depth_limit` or `directory_limit`). Absent
    /// when no cap was hit, and when the entry budget stopped it (`limit_hit` true, no named
    /// cause); older responses omit it too.
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub limit_reason: Option<ScanLimitReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub job_id: JobId,
    pub seq: Count,
    pub rev: Count,
    pub kind: JobKind,
    pub state: JobState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root_id: Option<RootId>,
    pub created_at: Timestamp,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub started_at: Option<Timestamp>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub finished_at: Option<Timestamp>,
    /// Running scans publish snapshots of their latest finished directory listings. Before
    /// enumeration starts this can be null; `done` carries the exact final report.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub progress: Option<ScanProgress>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub coverage: Option<CoverageState>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub error: Option<ErrorBody>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub duplicates: Option<DuplicateReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobRefRequest {
    pub job_id: JobId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobResult {
    pub job: Job,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobListRequest {
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub kinds: Option<UniqueVec<JobKind, 0, 4>>,
    pub limit: Int<1, 50>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobList {
    pub jobs: BoundedVec<Job, 0, 50>,
}
