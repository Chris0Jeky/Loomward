//! LW-137 on the wire: `volumes.list` carries the storage-path facts the Windows provider observed.
//! Unknown stays `null`, an unsupported or hidden health result is never healthy, and a provider
//! that cannot answer leaves the field absent. Synthetic providers and invented identifiers only.
#[cfg(windows)]
use super::*;
use crate::placement::wire_storage_path;
#[cfg(windows)]
use loomward_windows::storage_path::StorageQueryProvider;
use loomward_windows::storage_path::{
    HealthObservation, ReliabilityCounters, RemovalPolicy, StorageObservation, StoragePath,
    UsbTransport,
};
use loomward_windows::volumes::Fact;

fn known<T>(value: T) -> Fact<T> {
    Fact::Known { value }
}
fn unknown<T>() -> Fact<T> {
    Fact::Unknown {
        reason: "synthetic query unavailable".into(),
    }
}

#[cfg(windows)]
struct Synthetic(Option<StorageObservation>);
#[cfg(windows)]
impl StorageQueryProvider for Synthetic {
    fn query(&self, _: &std::path::Path) -> std::io::Result<StorageObservation> {
        self.0
            .clone()
            .ok_or_else(|| std::io::Error::other("synthetic provider unsupported"))
    }
}

fn path(bus: Fact<String>) -> StoragePath {
    StoragePath {
        volume_guid: r"\\?\Volume{00000000-0000-0000-0000-000000000876}\".into(),
        disk_model: unknown(),
        firmware: unknown(),
        bus_type: bus,
        usb_vid: unknown(),
        usb_pid: unknown(),
        bridge: unknown(),
        usb_transport: unknown(),
        logical_sector_bytes: unknown(),
        physical_sector_bytes: unknown(),
        removal_policy: unknown(),
        write_cache_enabled: unknown(),
        windows_build: unknown(),
    }
}

/// Counters, geometry, policy and cache all observed.
fn supported() -> StorageObservation {
    let mut p = path(known("nvme".into()));
    p.logical_sector_bytes = known(512);
    p.physical_sector_bytes = known(4096);
    p.removal_policy = known(RemovalPolicy::ExpectOrderlyRemoval);
    p.write_cache_enabled = known(true);
    StorageObservation {
        storage_path: p,
        mount_paths: known(vec![r"Q:\".into()]),
        health: HealthObservation::CountersPresent {
            counters: ReliabilityCounters {
                temperature_celsius: Some(31),
                read_errors_total: Some(0),
                power_on_hours: Some(123),
                ..Default::default()
            },
        },
    }
}

/// A USB bridge that hides the reliability counters and every fact behind it.
fn hidden_behind_bridge() -> StorageObservation {
    let mut p = path(known("usb".into()));
    p.bridge = known("Synthetic enclosure A".into());
    p.usb_transport = known(UsbTransport::Uas);
    p.logical_sector_bytes = known(512);
    StorageObservation {
        storage_path: p,
        mount_paths: unknown(),
        health: HealthObservation::UnsupportedOrHidden {
            reason: "synthetic USB bridge hides reliability counters".into(),
        },
    }
}

#[test]
fn wire_form_keeps_known_values_and_leaves_every_unobserved_fact_null() {
    let wire = wire_storage_path(&supported());
    assert_eq!(wire["bus_type"], "nvme");
    assert_eq!(wire["logical_sector_bytes"], 512);
    assert_eq!(wire["physical_sector_bytes"], 4096);
    assert_eq!(wire["removal_policy"], "expect_orderly_removal");
    assert_eq!(wire["write_cache_enabled"], true);
    assert!(wire["bridge"].is_null() && wire["usb_transport"].is_null());
    assert_eq!(wire["health"]["state"], "counters_present");
    // A measured zero stays zero; an absent property stays null, never zero.
    assert_eq!(wire["health"]["counters"]["read_errors_total"], 0);
    assert!(wire["health"]["counters"]["wear_percent"].is_null());
    assert!(wire["health"]["counters"]["write_errors_total"].is_null());
    // Nothing identifying travels: no GUID, no mount path.
    let text = wire.to_string();
    assert!(!text.contains("Volume{") && !text.contains("Q:"), "{text}");
}

#[test]
fn hidden_or_empty_health_is_unsupported_or_hidden_never_healthy() {
    let wire = wire_storage_path(&hidden_behind_bridge());
    assert_eq!(wire["bus_type"], "usb");
    assert_eq!(wire["bridge"], "Synthetic enclosure A");
    assert_eq!(wire["usb_transport"], "uas");
    assert!(wire["physical_sector_bytes"].is_null() && wire["write_cache_enabled"].is_null());
    assert_eq!(wire["health"]["state"], "unsupported_or_hidden");
    assert!(wire["health"]["counters"].is_null());
    assert!(wire["health"]["reason"]
        .as_str()
        .unwrap()
        .contains("bridge hides"));
    // An all-absent counter record normalises to unsupported_or_hidden in the provider; so does the wire.
    let mut empty = supported();
    empty.health = HealthObservation::from_counters(known(ReliabilityCounters::default()));
    assert_eq!(
        wire_storage_path(&empty)["health"]["state"],
        "unsupported_or_hidden"
    );
    // A device that reports an unknown bus is unknown, not a bus called "unknown".
    let mut odd = supported();
    odd.storage_path.bus_type = known("unknown".into());
    assert!(wire_storage_path(&odd)["bus_type"].is_null());
}

#[cfg(windows)]
#[test]
fn volumes_list_carries_the_storage_path_from_the_provider() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let list = |svc: &Service| ok("volumes.list", call(svc, "volumes.list", json!({}))).0;
    for (observed, bus, state, geometry) in [
        (supported(), "nvme", "counters_present", json!(4096)),
        (
            hidden_behind_bridge(),
            "usb",
            "unsupported_or_hidden",
            Value::Null,
        ),
    ] {
        *svc.inner.hooks.storage.lock().unwrap() = Some(Box::new(Synthetic(Some(observed))));
        let volumes = list(svc);
        let storage = &volumes["volumes"][0]["storage_path"];
        assert_eq!(storage["bus_type"], bus);
        assert_eq!(storage["physical_sector_bytes"], geometry);
        assert_eq!(storage["health"]["state"], state);
    }
    // A provider that cannot answer leaves the field absent: unknown, not healthy.
    *svc.inner.hooks.storage.lock().unwrap() = Some(Box::new(Synthetic(None)));
    let volumes = list(svc);
    assert!(volumes["volumes"][0].get("storage_path").is_none());
    // The real provider reports no health source yet, so whatever it observed is unsupported_or_hidden.
    *svc.inner.hooks.storage.lock().unwrap() = None;
    let volumes = list(svc);
    if let Some(storage) = volumes["volumes"][0].get("storage_path") {
        assert_eq!(storage["health"]["state"], "unsupported_or_hidden");
        assert!(storage["health"]["counters"].is_null());
    }
}
