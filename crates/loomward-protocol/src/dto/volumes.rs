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
