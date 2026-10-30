//! LW-137 on the wire: `volumes.list` carries the storage-path facts the Windows provider observed.
//! Unknown stays `null`, an unsupported or hidden health result is never healthy, and a provider
//! that cannot answer leaves the field absent. Synthetic providers and invented identifiers only.
#[cfg(windows)]
use super::*;
use crate::placement::wire_storage_path;
#[cfg(windows)]
use loomward_windows::storage_path::StorageQueryProvider;
use loomward_windows::storage_path::{
    CriticalWarning, HealthObservation, ReliabilityCounters, RemovalPolicy, StorageObservation,
    StoragePath, UsbTransport,
};
use loomward_windows::volumes::Fact;
use serde_json::json;

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

/// A disk whose counter read stalls for 30 s unless the caller's health budget ends the wait,
/// as the Windows provider does. Records every budget it is given.
#[cfg(windows)]
struct Stalled(std::sync::Arc<std::sync::Mutex<Vec<std::time::Duration>>>);
#[cfg(windows)]
impl StorageQueryProvider for Stalled {
    fn query(&self, root: &std::path::Path) -> std::io::Result<StorageObservation> {
        self.query_within(root, std::time::Duration::MAX)
    }
    fn query_within(
        &self,
        _: &std::path::Path,
        health_budget: std::time::Duration,
    ) -> std::io::Result<StorageObservation> {
        self.0.lock().unwrap().push(health_budget);
        let wait = health_budget.min(std::time::Duration::from_secs(30));
        std::thread::sleep(wait);
        let mut observed = supported();
        observed.health = HealthObservation::UnsupportedOrHidden {
            reason: "synthetic stall: gave up at the budget".into(),
        };
        Ok(observed)
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
    // Unreported NVMe fields are null, not a clear warning or a zero.
    for field in [
        "critical_warning",
        "available_spare_percent",
        "available_spare_threshold_percent",
        "media_errors",
        "unsafe_shutdowns",
    ] {
        assert!(wire["health"]["counters"][field].is_null(), "{field}");
    }
}

#[test]
fn nvme_critical_warning_spare_media_and_shutdowns_reach_the_wire_as_reported() {
    let mut observed = supported();
    observed.health = HealthObservation::CountersPresent {
        counters: ReliabilityCounters {
            critical_warning: Some(CriticalWarning::from_byte(0b1010_0101)),
            available_spare_percent: Some(0),
            available_spare_threshold_percent: Some(10),
            media_errors: Some(0),
            unsafe_shutdowns: Some(u64::MAX),
            ..Default::default()
        },
    };
    // The wire form parses as the protocol type, so it cannot drift from the contract.
    let parsed: loomward_protocol::VolumeStoragePath =
        serde_json::from_value(wire_storage_path(&observed)).unwrap();
    assert_eq!(
        parsed
            .health
            .counters
            .and_then(|c| c.critical_warning)
            .map(|w| (w.available_spare_low, w.reliability_degraded, w.other_bits)),
        Some((true, true, 0xA0))
    );
    let counters = wire_storage_path(&observed)["health"]["counters"].clone();
    assert_eq!(
        counters["critical_warning"],
        json!({
            "available_spare_low": true, "temperature": false, "reliability_degraded": true,
            "read_only": false, "volatile_backup_failed": false, "other_bits": 0xA0,
        })
    );
    // Measured zeros stay zero; a count past the wire's exact-integer range is clamped like the rest.
    assert_eq!(counters["available_spare_percent"], 0);
    assert_eq!(counters["available_spare_threshold_percent"], 10);
    assert_eq!(counters["media_errors"], 0);
    assert_eq!(counters["unsafe_shutdowns"], 9_007_199_254_740_991u64);
    // A clear byte is an object of false flags, distinct from the null of an unreported byte.
    observed.health = HealthObservation::CountersPresent {
        counters: ReliabilityCounters {
            critical_warning: Some(CriticalWarning::default()),
            ..Default::default()
        },
    };
    let clear = wire_storage_path(&observed)["health"]["counters"]["critical_warning"].clone();
    assert_eq!(clear["available_spare_low"], false);
    assert_eq!(clear["other_bits"], 0);
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

/// #1065 review: a stalled counter read waits only what the request deadline leaves, so the
/// listing still answers (health unknown) instead of failing with deadline_exceeded.
#[cfg(windows)]
#[test]
fn a_stalled_health_read_degrades_only_health_and_the_listing_still_answers() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    let budgets = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    *svc.inner.hooks.storage.lock().unwrap() = Some(Box::new(Stalled(budgets.clone())));
    let mut request = request("volumes.list", json!({}));
    request.deadline_ms = Some(loomward_protocol::Int::new(1_500).unwrap());
    let started = std::time::Instant::now();
    let (list, _) = ok("volumes.list", svc.call(request, &CallContext::http()));
    // The synthetic stall would have run 30 s; the budget ended it well inside the request.
    assert!(started.elapsed() < std::time::Duration::from_millis(1_500));
    let list = serde_json::to_value(list).unwrap();
    let storage = &list["volumes"][0]["storage_path"];
    assert_eq!(storage["health"]["state"], "unsupported_or_hidden");
    assert!(storage["health"]["counters"].is_null());
    assert_eq!(
        storage["bus_type"], "nvme",
        "the rest of the path is still reported"
    );
    let seen = budgets.lock().unwrap().clone();
    assert!(!seen.is_empty());
    // Never more than the deadline minus the reserve for finishing the response.
    assert!(
        seen.iter()
            .all(|b| *b <= std::time::Duration::from_millis(1_250)),
        "{seen:?}"
    );
}

#[test]
fn the_health_budget_is_what_the_deadline_leaves_minus_the_reserve() {
    use crate::placement::health_budget;
    use std::time::{Duration, Instant};
    let now = Instant::now();
    assert_eq!(
        health_budget(now + Duration::from_secs(2), now),
        Duration::from_millis(1_750)
    );
    assert_eq!(
        health_budget(now + Duration::from_millis(250), now),
        Duration::ZERO
    );
    assert_eq!(
        health_budget(now + Duration::from_millis(100), now),
        Duration::ZERO
    );
    assert_eq!(
        health_budget(now, now + Duration::from_secs(1)),
        Duration::ZERO
    );
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
    // The real provider reads NVMe counters where Windows allows it (#876); either way the wire is
    // consistent: counters only with counters_present, a reason only without.
    *svc.inner.hooks.storage.lock().unwrap() = None;
    let volumes = list(svc);
    if let Some(storage) = volumes["volumes"][0].get("storage_path") {
        let health = &storage["health"];
        match health["state"].as_str() {
            Some("counters_present") => {
                assert!(health["counters"].is_object() && health["reason"].is_null());
            }
            Some("unsupported_or_hidden") => {
                assert!(health["counters"].is_null() && health["reason"].is_string());
            }
            other => panic!("unexpected health state {other:?}"),
        }
    }
}
