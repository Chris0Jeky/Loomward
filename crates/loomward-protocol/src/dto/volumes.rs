//! Volumes and tiers. Mirrors `contracts/v3/view-service.schema.json`; the contract tests keep them equal.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceHintBusType {
    Nvme,
    Sata,
    Sas,
    Scsi,
    Usb,
    Raid,
    Virtual,
    Sd,
    Other,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceHintBasis {
    IoctlStorageQueryProperty,
    Unavailable,
}

/// A hint about the device, not a measured speed (docs/07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceHint {
    pub bus_type: DeviceHintBusType,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub seek_penalty: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub multiple_disks: Option<bool>,
    pub basis: DeviceHintBasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierInfoBasis {
    Declared,
    DeviceHint,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierInfo {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub tier: Option<Tier>,
    pub basis: TierInfoBasis,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub declared_tier: Option<Tier>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub hint_tier: Option<Tier>,
    pub note: Text<256>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeFeatures {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub file_ids_128: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub hard_links: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub sparse_files: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub compression: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reparse_points: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub usn_journal: Option<bool>,
}

/// LW-137: removal policy as the OS reports it. `null` on the wire is unknown, never a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageRemovalPolicy {
    ExpectNoRemoval,
    ExpectOrderlyRemoval,
    ExpectSurpriseRemoval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageUsbTransport {
    Bot,
    Uas,
}

/// Individual reliability properties the device reported; an absent one is `null`, never zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageCounters {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub temperature_celsius: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub read_errors_total: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub read_errors_uncorrected: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub write_errors_total: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub write_errors_uncorrected: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub power_on_hours: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub wear_percent: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub start_stop_cycles: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub critical_warning: Option<StorageCriticalWarning>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub available_spare_percent: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub available_spare_threshold_percent: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub media_errors: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub unsafe_shutdowns: Option<Count>,
}

/// The NVMe Critical Warning byte as the device latched it: a device report, not a Loomward verdict.
/// A clear byte is not "healthy" either. `null` (the whole object) is unknown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageCriticalWarning {
    pub available_spare_low: bool,
    pub temperature: bool,
    pub reliability_degraded: bool,
    pub read_only: bool,
    pub volatile_backup_failed: bool,
    /// Bits 5-7 of the byte, unshifted and uninterpreted (0..=255).
    pub other_bits: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageHealthState {
    CountersPresent,
    UnsupportedOrHidden,
}

/// Reported counters are evidence, not a verdict: there is no healthy state. `unsupported_or_hidden`
/// (including SMART behind a USB bridge) is unknown, and a client must never present it as healthy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageHealth {
    pub state: StorageHealthState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reason: Option<Text<256>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub counters: Option<StorageCounters>,
}

/// LW-137 storage-path facts for one volume. Every `null` is unknown, never a default; the volume
/// GUID, device serial and mount paths are not carried. Observation only, never authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeStoragePath {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub bus_type: Option<Text<32>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub bridge: Option<Text<64>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub usb_transport: Option<StorageUsbTransport>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub logical_sector_bytes: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub physical_sector_bytes: Option<Count>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub removal_policy: Option<StorageRemovalPolicy>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub write_cache_enabled: Option<bool>,
    pub health: StorageHealth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Volume {
    pub volume_id: VolumeId,
    pub display_name: Text<64>,
    pub mount_points: BoundedVec<Text<260>, 0, 16>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub filesystem: Option<Text<32>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub label: Option<Text<64>>,
    pub online: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub read_only: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub removable: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub capacity_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub free_bytes: Option<Bytes>,
    pub device: DeviceHint,
    pub tier: TierInfo,
    pub features: VolumeFeatures,
    /// Absent when the service did not observe the storage path (offline, no active grant, query
    /// refused); absent is unknown, never healthy. Additive: older services omit it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::types::optional_present"
    )]
    pub storage_path: Option<VolumeStoragePath>,
    pub observed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeList {
    pub volumes: BoundedVec<Volume, 0, 64>,
}

/// Records a human preference. null clears the declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclareTierRequest {
    pub volume_id: VolumeId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub tier: Option<Tier>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclareTierResult {
    pub volume: Volume,
    pub preference_recorded_at: Timestamp,
}

/// Absolute owner preference; null clears it. Requires expected_state_rev on the envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclareReserveRequest {
    pub volume_id: VolumeId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reserve_bytes: Option<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclareReserveResult {
    pub volume_id: VolumeId,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reserve_bytes: Option<Bytes>,
    pub preference_recorded_at: Timestamp,
}
