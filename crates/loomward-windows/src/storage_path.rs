//! Read-only storage-path observations (LW-137 / ADR R6), never operation authority.
use crate::volumes::Fact;
use serde::Serialize;
use std::{io, path::Path, time::Duration};

/// Qualification attaches to this tuple, not to mount paths or a drive letter.
/// Device serials in temporary IOCTL response buffers are never extracted, retained or hashed.
/// The volume GUID identifies the volume; observed enclosure/firmware/policy changes invalidate
/// qualification. No serial is needed for this volume-level identity.
/// Unknown fields mean the tuple cannot prove an unchanged physical path. `==` compares two
/// observations as recorded (equal unknowns compare equal); qualification must use
/// [`StoragePath::proves_same_path`], never `==` (review of #983).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoragePath {
    pub volume_guid: String,
    pub disk_model: Fact<String>,
    pub firmware: Fact<String>,
    pub bus_type: Fact<String>,
    pub usb_vid: Fact<u16>,
    pub usb_pid: Fact<u16>,
    pub bridge: Fact<String>,
    pub usb_transport: Fact<UsbTransport>,
    pub logical_sector_bytes: Fact<u32>,
    pub physical_sector_bytes: Fact<u32>,
    pub removal_policy: Fact<RemovalPolicy>,
    pub write_cache_enabled: Fact<bool>,
    pub windows_build: Fact<u32>,
}

impl StoragePath {
    /// Imported path qualification is evidence only for the same fully observed tuple.
    /// This never creates qualification; LW-134 has not supplied any production evidence.
    pub fn requalify_receipt(
        &self,
        current: &Self,
        receipt: &mut loomward_operations::CopyReceipt,
    ) {
        if !self.proves_same_path(current) {
            receipt.downgrade_power_loss();
        }
    }

    /// Whether `other` is provably the same physical path: same volume and every field known and
    /// equal in both. Any unknown field proves nothing, so a changed enclosure cannot hide.
    pub fn proves_same_path(&self, other: &Self) -> bool {
        fn same<T: PartialEq>(a: &Fact<T>, b: &Fact<T>) -> bool {
            matches!((a, b), (Fact::Known { value: x }, Fact::Known { value: y }) if x == y)
        }
        self.volume_guid == other.volume_guid
            && same(&self.disk_model, &other.disk_model)
            && same(&self.firmware, &other.firmware)
            && same(&self.bus_type, &other.bus_type)
            && same(&self.usb_vid, &other.usb_vid)
            && same(&self.usb_pid, &other.usb_pid)
            && same(&self.bridge, &other.bridge)
            && same(&self.usb_transport, &other.usb_transport)
            && same(&self.logical_sector_bytes, &other.logical_sector_bytes)
            && same(&self.physical_sector_bytes, &other.physical_sector_bytes)
            && same(&self.removal_policy, &other.removal_policy)
            && same(&self.write_cache_enabled, &other.write_cache_enabled)
            && same(&self.windows_build, &other.windows_build)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbTransport {
    Bot,
    Uas,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalPolicy {
    ExpectNoRemoval,
    ExpectOrderlyRemoval,
    ExpectSurpriseRemoval,
}

/// Individual absent MSFT_StorageReliabilityCounter properties stay unknown, not zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReliabilityCounters {
    pub temperature_celsius: Option<u32>,
    pub read_errors_total: Option<u64>,
    pub read_errors_uncorrected: Option<u64>,
    pub write_errors_total: Option<u64>,
    pub write_errors_uncorrected: Option<u64>,
    pub power_on_hours: Option<u64>,
    pub wear_percent: Option<u32>,
    pub start_stop_cycles: Option<u64>,
}

/// Presence of counters is evidence, not a health verdict or SMART passthrough claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HealthObservation {
    CountersPresent { counters: ReliabilityCounters },
    UnsupportedOrHidden { reason: String },
}

impl HealthObservation {
    pub fn from_counters(counters: Fact<ReliabilityCounters>) -> Self {
        match counters {
            Fact::Known { value } if value != ReliabilityCounters::default() => {
                Self::CountersPresent { counters: value }
            }
            Fact::Known { .. } => Self::UnsupportedOrHidden {
                reason: "empty reliability counter result".into(),
            },
            Fact::Unknown { reason } => Self::UnsupportedOrHidden { reason },
        }
    }
}

#[cfg(any(windows, test))]
pub(crate) fn sector_alignment(logical: u32, physical: u32) -> io::Result<usize> {
    if !logical.is_power_of_two()
        || !physical.is_power_of_two()
        || physical < logical
        || physical > 1024 * 1024
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown sector alignment",
        ));
    }
    Ok(physical as usize)
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageObservation {
    pub storage_path: StoragePath,
    /// Presentation only; excluded from the storage-path identity tuple.
    pub mount_paths: Fact<Vec<String>>,
    pub health: HealthObservation,
}

/// One explicitly selected path; providers do not scan disks or change device policy.
pub trait StorageQueryProvider {
    fn query(&self, root: &Path) -> io::Result<StorageObservation>;

    /// Like [`query`](Self::query), but the slow reliability-counter read may wait at most
    /// `health_budget` (zero skips it). A stalled counter then degrades only `health`, so a caller
    /// with a request deadline is never held past it. Providers without a slow read ignore it.
    fn query_within(&self, root: &Path, health_budget: Duration) -> io::Result<StorageObservation> {
        let _ = health_budget;
        self.query(root)
    }
}

pub struct WindowsStorageQueryProvider;

impl StorageQueryProvider for WindowsStorageQueryProvider {
    fn query(&self, root: &Path) -> io::Result<StorageObservation> {
        self.query_within(root, Duration::MAX)
    }

    fn query_within(&self, root: &Path, health_budget: Duration) -> io::Result<StorageObservation> {
        #[cfg(windows)]
        {
            crate::volumes::storage_query(root, health_budget)
        }
        #[cfg(not(windows))]
        {
            let _ = (root, health_budget);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows storage queries unavailable",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known<T>(value: T) -> Fact<T> {
        Fact::Known { value }
    }
    fn unknown<T>() -> Fact<T> {
        Fact::Unknown {
            reason: "synthetic query unavailable".into(),
        }
    }

    #[derive(Clone)]
    struct SyntheticProvider {
        path: StoragePath,
        mounts: Vec<String>,
        counters: Fact<ReliabilityCounters>,
    }
    impl StorageQueryProvider for SyntheticProvider {
        fn query(&self, _: &Path) -> io::Result<StorageObservation> {
            Ok(StorageObservation {
                storage_path: self.path.clone(),
                mount_paths: known(self.mounts.clone()),
                health: HealthObservation::from_counters(self.counters.clone()),
            })
        }
    }
    fn fixture(counters: Fact<ReliabilityCounters>) -> SyntheticProvider {
        SyntheticProvider {
            path: StoragePath {
                volume_guid: r"\\?\Volume{00000000-0000-0000-0000-000000000137}\".into(),
                disk_model: known("Synthetic disk".into()),
                firmware: known("fixture-v1".into()),
                bus_type: known("usb".into()),
                usb_vid: known(0xffff),
                usb_pid: known(0xffff),
                bridge: known("Synthetic enclosure A".into()),
                usb_transport: known(UsbTransport::Uas),
                logical_sector_bytes: known(512),
                physical_sector_bytes: known(4096),
                removal_policy: known(RemovalPolicy::ExpectOrderlyRemoval),
                write_cache_enabled: known(true),
                windows_build: known(99999),
            },
            mounts: vec![r"Q:\".into()],
            counters,
        }
    }
    fn observe(provider: &impl StorageQueryProvider) -> StorageObservation {
        provider.query(Path::new("synthetic-root")).unwrap()
    }

    #[test]
    fn reliability_counters_present_preserve_values_and_unknown_properties() {
        let counters = ReliabilityCounters {
            temperature_celsius: Some(31),
            read_errors_total: Some(0),
            write_errors_total: Some(2),
            power_on_hours: Some(123),
            wear_percent: Some(4),
            start_stop_cycles: Some(9),
            ..Default::default()
        };
        let observed = observe(&fixture(known(counters.clone())));
        assert_eq!(
            observed.health,
            HealthObservation::CountersPresent { counters }
        );
        let json = serde_json::to_value(observed).unwrap();
        assert!(json["health"]["counters"]["read_errors_uncorrected"].is_null());
        assert_eq!(json["health"]["counters"]["read_errors_total"], 0);
    }

    #[test]
    fn empty_reliability_result_is_unsupported_or_hidden_never_healthy() {
        for counters in [unknown(), known(ReliabilityCounters::default())] {
            let health = observe(&fixture(counters)).health;
            assert!(matches!(
                health,
                HealthObservation::UnsupportedOrHidden { .. }
            ));
            assert_eq!(
                serde_json::to_value(health).unwrap()["state"],
                "unsupported_or_hidden"
            );
        }
    }

    #[test]
    fn bridge_hiding_smart_is_unknown() {
        let provider = fixture(Fact::Unknown {
            reason: "synthetic USB bridge hides reliability counters".into(),
        });
        assert!(matches!(observe(&provider).health,
            HealthObservation::UnsupportedOrHidden { reason } if reason.contains("bridge hides")));
    }

    #[test]
    fn receipt_qualification_is_bound_to_the_fully_known_storage_path() {
        use loomward_operations::{CopyReceipt, OpState, ReceiptLabel};
        let original = observe(&fixture(unknown())).storage_path;
        let qualified = || {
            // Synthetic future LW-134 evidence, never produced by today's copy engine.
            let mut receipt = CopyReceipt::from_copy(OpState::Committed, 1, 1, true);
            receipt.highest_label = Some(ReceiptLabel::PowerLossQualified);
            receipt
        };
        let mut receipt = qualified();
        original.requalify_receipt(&original, &mut receipt);
        assert_eq!(
            receipt.highest_label,
            Some(ReceiptLabel::PowerLossQualified)
        );
        let mut changed = original.clone();
        changed.bridge = known("Synthetic enclosure B".into());
        let mut firmware = original.clone();
        firmware.firmware = known("fixture-v2".into());
        let mut partial = original.clone();
        partial.bridge = unknown();
        for current in [changed, firmware, partial] {
            let mut receipt = qualified();
            original.requalify_receipt(&current, &mut receipt);
            assert_eq!(receipt.highest_label, Some(ReceiptLabel::ReadBack));
            assert!(receipt
                .explanation
                .contains("not power-loss tested on this disk"));
            assert!(!serde_json::to_string(&receipt).unwrap().contains("durable"));
        }
        let mut partial = original;
        partial.firmware = unknown();
        let mut receipt = qualified();
        partial.requalify_receipt(&partial, &mut receipt);
        assert_eq!(receipt.highest_label, Some(ReceiptLabel::ReadBack));
    }

    #[test]
    fn unknown_fields_never_prove_the_same_path() {
        let full = observe(&fixture(unknown())).storage_path;
        assert!(full.proves_same_path(&full.clone()));
        let mut swapped = full.clone();
        swapped.bridge = known("Synthetic enclosure B".into());
        assert!(!full.proves_same_path(&swapped));
        // Unknown bridge and firmware: equal as recorded, but they prove nothing about the enclosure.
        let mut partial = full.clone();
        partial.bridge = unknown();
        partial.firmware = unknown();
        assert_eq!(partial, partial.clone());
        assert!(!partial.proves_same_path(&partial.clone()));
    }

    #[test]
    fn letter_change_keeps_identity_but_enclosure_firmware_and_policy_change_it() {
        let provider = fixture(unknown());
        let original = observe(&provider);
        let mut changed = provider.clone();
        changed.mounts = vec![r"R:\".into()];
        let remounted = observe(&changed);
        assert_ne!(original.mount_paths, remounted.mount_paths);
        assert_eq!(original.storage_path, remounted.storage_path);
        changed.path.bridge = known("Synthetic enclosure B".into());
        assert_ne!(original.storage_path, observe(&changed).storage_path);
        changed = provider.clone();
        changed.path.firmware = known("fixture-v2".into());
        assert_ne!(original.storage_path, observe(&changed).storage_path);
        changed = provider;
        changed.path.removal_policy = known(RemovalPolicy::ExpectSurpriseRemoval);
        assert_ne!(original.storage_path, observe(&changed).storage_path);
    }

    #[test]
    fn sector_geometry_and_observed_policy_cache_are_preserved_including_failed_reads() {
        let mut provider = fixture(unknown());
        for logical in [512, 4096] {
            provider.path.logical_sector_bytes = known(logical);
            let path = observe(&provider).storage_path;
            assert_eq!(path.logical_sector_bytes, known(logical));
            assert_eq!(path.physical_sector_bytes, known(4096));
            assert_eq!(
                path.removal_policy,
                known(RemovalPolicy::ExpectOrderlyRemoval)
            );
            assert_eq!(path.write_cache_enabled, known(true));
        }
        provider.path.removal_policy = unknown();
        provider.path.write_cache_enabled = unknown();
        let path = observe(&provider).storage_path;
        assert!(matches!(path.removal_policy, Fact::Unknown { .. }));
        assert!(matches!(path.write_cache_enabled, Fact::Unknown { .. }));
    }

    #[test]
    fn native_alignment_read_accepts_512e_and_4kn_and_rejects_invalid_geometry() {
        assert_eq!(sector_alignment(512, 4096).unwrap(), 4096);
        assert_eq!(sector_alignment(4096, 4096).unwrap(), 4096);
        for (logical, physical) in [
            (0, 4096),
            (512, 0),
            (513, 4096),
            (4096, 512),
            (512, 2 * 1024 * 1024),
        ] {
            assert!(sector_alignment(logical, physical).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn real_provider_refuses_elevation_before_querying() {
        let _reset = crate::enumerate::force_elevation_for_test();
        assert_eq!(
            WindowsStorageQueryProvider
                .query(Path::new("synthetic-root"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[cfg(windows)]
    #[test]
    fn real_provider_reads_selected_temporary_volume_without_inventing_missing_data() {
        // The provider refuses an elevated token; hosted Windows runners are elevated (#823).
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let observation = WindowsStorageQueryProvider.query(root.path()).unwrap();
        assert!(observation
            .storage_path
            .volume_guid
            .starts_with(r"\\?\Volume{"));
        assert!(matches!(
            observation.storage_path.removal_policy,
            Fact::Unknown { .. }
        ));
        assert!(matches!(
            observation.storage_path.bridge,
            Fact::Unknown { .. }
        ));
        if let (Fact::Known { value: logical }, Fact::Known { value: physical }) = (
            observation.storage_path.logical_sector_bytes,
            observation.storage_path.physical_sector_bytes,
        ) {
            sector_alignment(logical, physical).unwrap();
        }
    }

    /// #876: whatever this host's disk exposes, the health field is evidence or a reason, within
    /// the query budget. No value, serial or model is asserted or printed.
    #[cfg(windows)]
    #[test]
    fn real_provider_reports_counters_or_a_reason_within_the_query_bound() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let observation = WindowsStorageQueryProvider.query(root.path()).unwrap();
        let bound = crate::reliability::QUERY_BUDGET + std::time::Duration::from_secs(3);
        assert!(started.elapsed() < bound);
        match observation.health {
            HealthObservation::CountersPresent { counters } => {
                assert_ne!(counters, ReliabilityCounters::default());
            }
            HealthObservation::UnsupportedOrHidden { reason } => {
                assert!(!reason.is_empty() && !reason.contains("Volume{"));
            }
        }
    }

    /// A request with no time left skips the slow counter read on every bus: health is a reason,
    /// the rest of the observation is still returned.
    #[cfg(windows)]
    #[test]
    fn real_provider_with_no_budget_skips_the_counter_read() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let observation = WindowsStorageQueryProvider
            .query_within(root.path(), Duration::ZERO)
            .unwrap();
        assert!(observation
            .storage_path
            .volume_guid
            .starts_with(r"\\?\Volume{"));
        assert!(matches!(
            observation.health,
            HealthObservation::UnsupportedOrHidden { .. }
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(not(windows))]
    #[test]
    fn real_provider_is_explicitly_unsupported_off_windows() {
        assert_eq!(
            WindowsStorageQueryProvider
                .query(Path::new("synthetic-root"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
    }
}
