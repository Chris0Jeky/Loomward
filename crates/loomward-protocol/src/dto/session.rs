//! Session and health. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesObservation {
    pub metadata_scan: bool,
    pub process_observation: bool,
    pub gpu_observation: bool,
    pub disk_io_observation: bool,
    pub teacher_disclosure: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesEffects {
    pub file_move: ConstFalse,
    pub file_delete: ConstFalse,
    pub file_rename: ConstFalse,
    pub file_write: ConstFalse,
    pub content_read: ConstFalse,
    pub process_kill: ConstFalse,
    pub process_suspend: ConstFalse,
    pub process_priority: ConstFalse,
    pub memory_trim: ConstFalse,
    pub uninstall: ConstFalse,
    pub elevation: ConstFalse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub observation: CapabilitiesObservation,
    pub effects: CapabilitiesEffects,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfoFeatures {
    pub grant_picker: bool,
    pub disclosure_dialog: bool,
    pub teacher_available: bool,
    pub telemetry_available: bool,
    pub gpu_available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfoLimits {
    pub max_request_bytes: Count,
    pub max_slice_nodes: Count,
    pub max_page_items: Count,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfo {
    pub protocol: Protocol,
    pub engine_version: Text<64>,
    pub adapter: Adapter,
    pub dataset_class: DatasetClass,
    pub session_started_at: Timestamp,
    pub enumeration_strategy: EnumerationStrategy,
    pub capabilities: Capabilities,
    pub features: SessionInfoFeatures,
    pub limits: SessionInfoLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthWarningCode {
    CatalogWriterBacklog,
    LowStateDisk,
    ScanThrottled,
    TelemetryUnavailable,
    TeacherFailed,
    RootIdentityChanged,
    EventStreamLagging,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthWarning {
    pub code: HealthWarningCode,
    pub message: Text<512>,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthEngine {
    /// PROCESS_MEMORY_COUNTERS_EX2.PrivateUsage.
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub private_commit_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub working_set_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cpu_seconds: Option<Rate>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub threads: Option<Count>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthCatalog {
    pub schema_version: Int<1, 9007199254740991>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub db_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub wal_bytes: Option<Bytes>,
    pub files: Count,
    pub dirs: Count,
    pub writer_queue_depth: Count,
    pub writer_queue_capacity: Count,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub observed_at: Timestamp,
    pub engine: HealthEngine,
    pub catalog: HealthCatalog,
    pub jobs_running: Count,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub last_error: Option<ErrorBody>,
    pub warnings: BoundedVec<HealthWarning, 0, 32>,
}
