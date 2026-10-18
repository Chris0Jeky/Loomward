//! Redacted diagnostic bundles (LW-055, owner decision q-15, docs/42 ADR-V3-24). Mirrors
//! `contracts/v3/view-service.schema.json`; the contract tests keep them equal.
//!
//! The bundle is an allowlist by construction: every field is typed here and every struct is
//! `deny_unknown_fields`, so a path, name, label, argument or message cannot be added to it
//! without a contract change. There is no free-text field except the fixed unknown reasons.

use super::*;
use crate::ErrorCode;

/// A value the service could not read: visible, never omitted and never zero (invariant 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticUnknown {
    pub unknown: UnknownReason,
}

/// Either the observed value or an explicit unknown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Known<T> {
    Value(T),
    Unknown(DiagnosticUnknown),
}

impl<T> Known<T> {
    /// `reason` must be a fixed string, never input.
    pub fn unknown(reason: &'static str) -> Self {
        Known::Unknown(DiagnosticUnknown {
            unknown: UnknownReason::new(reason).expect("a snake_case reason"),
        })
    }
}

const_str!(
    pub DiagnosticsFormat = "loomward-diagnostics/1"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsBuild {
    pub engine_version: Text<64>,
    pub protocol: Protocol,
    pub adapter: Adapter,
    pub dataset_class: DatasetClass,
    pub enumeration_strategy: EnumerationStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsOsFamily {
    Windows,
    Other,
}

/// Owner decision q-16 allows host hardware in new evidence; the bundle keeps it to these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsHost {
    pub os_family: DiagnosticsOsFamily,
    pub os_build: Known<Count>,
    pub logical_processors: Known<Count>,
    pub memory_total_bytes: Known<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsSession {
    pub capabilities: Capabilities,
    pub features: SessionInfoFeatures,
}

/// `health.get` without its messages: warning codes only, never their text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsHealth {
    pub engine_private_commit_bytes: Known<Bytes>,
    /// Whole milliseconds: the bundle holds no floating-point numbers, so its text is canonical.
    pub engine_cpu_ms: Known<Count>,
    pub catalog_db_bytes: Known<Bytes>,
    pub catalog_wal_bytes: Known<Bytes>,
    pub writer_queue_depth: Count,
    pub writer_queue_capacity: Count,
    pub jobs_running: Count,
    pub warning_codes: BoundedVec<HealthWarningCode, 0, 32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsStores {
    pub catalog_schema_version: Known<Count>,
    pub state_schema_version: Known<Count>,
    pub catalog_files: Known<Count>,
    pub catalog_dirs: Known<Count>,
}

/// Counts only: never a collection name, a label or a receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsCounts {
    pub roots_active: Count,
    pub roots_revoked: Count,
    pub roots_identity_changed: Count,
    pub roots_forgotten: Count,
    pub collections: Count,
    pub human_feedback_records: Count,
    pub teacher_label_records: Count,
    pub backup_evidence: Count,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsJobCount {
    pub kind: JobKind,
    pub state: JobState,
    pub count: Count,
}

/// Counted over the newest `window` jobs the engine lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsJobs {
    pub window: Count,
    pub by_kind_state: BoundedVec<DiagnosticsJobCount, 0, 24>,
}

/// The filesystem family, never the volume's name, label or serial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsFilesystem {
    Ntfs,
    Refs,
    Fat,
    Fat32,
    Exfat,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsIdentityMode {
    Verified,
    Unverified,
}

/// One root, keyed only by its opaque ID: no path, name or volume label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsRoot {
    pub root_id: RootId,
    pub origin: RootOrigin,
    pub grant_state: RootGrantState,
    pub scan_state: RootScanStateState,
    pub coverage: CoverageState,
    pub filesystem: Known<DiagnosticsFilesystem>,
    pub identity_mode: DiagnosticsIdentityMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsBundle {
    pub format: DiagnosticsFormat,
    pub generated_at: Timestamp,
    pub loomward: DiagnosticsBuild,
    pub host: DiagnosticsHost,
    pub session: DiagnosticsSession,
    pub health: Known<DiagnosticsHealth>,
    pub stores: DiagnosticsStores,
    pub counts: Known<DiagnosticsCounts>,
    pub jobs: Known<DiagnosticsJobs>,
    pub roots: Known<BoundedVec<DiagnosticsRoot, 0, 64>>,
    pub recent_errors: Known<BoundedVec<ErrorCode, 0, 32>>,
}

/// The bundle, plus the size and digest of the exact UTF-8 bytes `diagnostics.save` would write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsPreviewResult {
    pub bundle: DiagnosticsBundle,
    pub bytes: Bytes,
    pub digest: Digest,
}

/// Tauri adapter only. Rust opens the native save dialog; the webview never supplies a path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsSaveRequest {
    /// The digest of the preview the owner saw; anything else writes nothing.
    pub digest: Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsSaveOutcome {
    Saved,
    CancelledByUser,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsSaveRefusal {
    PreviewChanged,
    /// Saving is refused while Loomward runs with an elevated Windows token.
    ElevatedSession,
    UnsupportedTarget,
    /// An existing target that is a cloud placeholder (or other offline file): saving would recall it.
    CloudPlaceholder,
    WriteFailed,
    /// Another dialog was open, so the save dialog did not open.
    DialogBusy,
    /// The save dialog could not open (no owner window yet, no start folder, or no dialog on this adapter).
    DialogUnavailable,
    /// No path was returned; the dialog backend cannot distinguish cancellation from failure.
    DialogNoSelection,
}

/// Never the chosen path: it stays in Rust.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsSaveResult {
    pub outcome: DiagnosticsSaveOutcome,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub bytes_written: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub refusal: Option<DiagnosticsSaveRefusal>,
}
