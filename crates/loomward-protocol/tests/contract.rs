//! L1b: every example validates against its `$def` (JSON Schema) and round-trips through its
//! Rust DTO to an equal `serde_json::Value`; every negative fails in both; the schema keeps
//! its structural promises (closed objects, constant-false effects, no path-taking command).

mod common;

use common::registry::roundtrip;

#[test]
fn project_generation_precondition_requires_one_add_and_no_removals() {
    let base = serde_json::json!({"protocol":"loomward/3","request_id":"projects_guard",
        "command":"collections.update_members","expected_generation":"12","expected_state_rev":"3",
        "payload":{"collection_id":"co_1","add":["nd_1"],"remove":[]}});
    let valid: RequestEnvelope = serde_json::from_value(base.clone()).unwrap();
    assert!(valid.validate().is_ok());
    for payload in [
        serde_json::json!({"collection_id":"co_1","add":["nd_1","nd_2"],"remove":[]}),
        serde_json::json!({"collection_id":"co_1","add":["nd_1"],"remove":["nd_2"]}),
        serde_json::json!({"collection_id":"co_1","add":["nd_1"],"remove":[],"remove_members":["om_2"]}),
    ] {
        let mut invalid = base.clone();
        invalid["payload"] = payload;
        assert!(serde_json::from_value::<RequestEnvelope>(invalid)
            .unwrap()
            .validate()
            .is_err());
    }
}
use jsonschema::Validator;
use loomward_protocol::{
    decode_exact, Command, EventEnvelope, EventName, RequestEnvelope, ResponseEnvelope,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn alert_history_is_closed_bounded_and_lossless() {
    let mut schema = Schema::load();
    let mut history = read(&contracts().join("examples/commands/alerts.history.result.json"));
    assert!(schema.errors("AlertHistory", &history).is_empty());
    let decoded: loomward_protocol::AlertHistory = serde_json::from_value(history.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), history);
    for bytes in [Value::Null, json!("0"), json!("9007199254740993")] {
        history["samples"][0]["logical_bytes"] = bytes;
        assert!(schema.errors("AlertHistory", &history).is_empty());
        assert!(serde_json::from_value::<loomward_protocol::AlertHistory>(history.clone()).is_ok());
    }
    history["samples"][0]
        .as_object_mut()
        .unwrap()
        .remove("logical_bytes");
    assert!(!schema.errors("AlertHistory", &history).is_empty());
    assert!(serde_json::from_value::<loomward_protocol::AlertHistory>(history.clone()).is_err());
    history["samples"][0]["logical_bytes"] = json!(0);
    assert!(!schema.errors("AlertHistory", &history).is_empty());
    assert!(serde_json::from_value::<loomward_protocol::AlertHistory>(history.clone()).is_err());
    history["samples"][0]["logical_bytes"] = json!("0");
    history["samples"] = json!(vec![history["samples"][0].clone(); 65]);
    assert!(!schema.errors("AlertHistory", &history).is_empty());
    assert!(serde_json::from_value::<loomward_protocol::AlertHistory>(history).is_err());
    for bad in [
        json!({}),
        json!({"root_id":"rt_1","path":"private"}),
        json!({"root_id":"rt_1","limit":65}),
    ] {
        assert!(!schema.errors("AlertHistoryRequest", &bad).is_empty());
        assert!(serde_json::from_value::<loomward_protocol::AlertHistoryRequest>(bad).is_err());
    }
    assert_eq!(
        Command::AlertsHistory.adapters(),
        &[
            loomward_protocol::Adapter::Tauri,
            loomward_protocol::Adapter::Http
        ]
    );
}

fn contracts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/v3")
}

#[test]
fn sampled_system_cpu_count_is_required_but_nullable() {
    let mut schema = Schema::load();
    let mut value = read(&contracts().join("examples/commands/telemetry.snapshot.result.json"))
        ["system"]
        .clone();
    assert!(value.is_object());
    for cpus in [json!(16), Value::Null] {
        value["cpu"]["logical_cpus"] = cpus;
        assert!(schema.errors("SystemSample", &value).is_empty());
        let decoded: loomward_protocol::SystemSample =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
    value["cpu"].as_object_mut().unwrap().remove("logical_cpus");
    assert!(!schema.errors("SystemSample", &value).is_empty());
    assert!(serde_json::from_value::<loomward_protocol::SystemSample>(value).is_err());
}

#[test]
fn volume_storage_path_is_additive_and_never_reads_as_healthy() {
    let mut schema = Schema::load();
    let list = read(&contracts().join("examples/commands/volumes.list.result.json"));
    let volume = list["volumes"][1].clone();
    assert_eq!(
        volume["storage_path"]["health"]["state"],
        "unsupported_or_hidden"
    );
    assert!(schema.errors("Volume", &volume).is_empty());
    assert_eq!(roundtrip("Volume", &volume).unwrap().unwrap(), volume);
    // Additive: a row without the field is still valid (older services), and stays without it.
    let mut absent = volume.clone();
    absent.as_object_mut().unwrap().remove("storage_path");
    assert!(schema.errors("Volume", &absent).is_empty());
    assert_eq!(roundtrip("Volume", &absent).unwrap().unwrap(), absent);
    // A null storage_path is not a spelling of "absent", and no health state means healthy.
    let mut null = volume.clone();
    null["storage_path"] = Value::Null;
    assert!(!schema.errors("Volume", &null).is_empty());
    assert!(roundtrip("Volume", &null).unwrap().is_err());
    for state in ["healthy", "ok", "good"] {
        let mut healthy = volume.clone();
        healthy["storage_path"]["health"]["state"] = json!(state);
        assert!(!schema.errors("Volume", &healthy).is_empty());
        assert!(roundtrip("Volume", &healthy).unwrap().is_err());
    }
    // Every fact and counter is required-nullable: omission is refused, null is unknown.
    for (pointer, field) in [
        ("/storage_path", "bridge"),
        ("/storage_path", "removal_policy"),
        ("/storage_path/health", "reason"),
    ] {
        let mut missing = volume.clone();
        missing
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(!schema.errors("Volume", &missing).is_empty(), "{field}");
        assert!(roundtrip("Volume", &missing).unwrap().is_err(), "{field}");
    }
    let mut counters = list["volumes"][0]["storage_path"]["health"]["counters"].clone();
    // The NVMe fields are required-nullable too; a null critical warning is unknown, not clear.
    assert_eq!(counters["critical_warning"]["available_spare_low"], true);
    for field in [
        "critical_warning",
        "available_spare_percent",
        "available_spare_threshold_percent",
        "media_errors",
        "unsafe_shutdowns",
    ] {
        let mut unknown = counters.clone();
        unknown[field] = Value::Null;
        assert!(
            schema.errors("StorageCounters", &unknown).is_empty(),
            "{field}"
        );
        assert_eq!(
            roundtrip("StorageCounters", &unknown).unwrap().unwrap(),
            unknown
        );
        unknown.as_object_mut().unwrap().remove(field);
        assert!(
            !schema.errors("StorageCounters", &unknown).is_empty(),
            "{field}"
        );
        assert!(
            roundtrip("StorageCounters", &unknown).unwrap().is_err(),
            "{field}"
        );
    }
    // The warning is closed: an unnamed flag or a partial object is refused.
    for edit in ["extra", "missing", "wide"] {
        let mut warning = counters["critical_warning"].clone();
        match edit {
            "extra" => warning["healthy"] = json!(true),
            "missing" => {
                warning.as_object_mut().unwrap().remove("read_only");
            }
            _ => warning["other_bits"] = json!(256),
        }
        assert!(
            !schema.errors("StorageCriticalWarning", &warning).is_empty(),
            "{edit}"
        );
        assert!(
            roundtrip("StorageCriticalWarning", &warning)
                .unwrap()
                .is_err(),
            "{edit}"
        );
    }
    counters.as_object_mut().unwrap().remove("wear_percent");
    assert!(!schema.errors("StorageCounters", &counters).is_empty());
    assert!(roundtrip("StorageCounters", &counters).unwrap().is_err());
}

#[test]
fn reclaim_sizes_are_required_nullable_and_requests_are_bounded() {
    let mut schema = Schema::load();
    let plan = read(&contracts().join("examples/commands/reclaim.plan.result.json"));
    for (def, value, fields) in [
        (
            "ReclaimVolume",
            &plan["volume"],
            &["capacity_bytes", "free_bytes", "reserve_bytes"][..],
        ),
        (
            "CoveredRoot",
            &plan["covered_roots"][0],
            &["allocated_bytes"][..],
        ),
        (
            "ReclaimCandidate",
            &plan["groups"][0]["candidates"][0],
            &["logical_bytes", "allocated_bytes", "reclaimable_bytes"][..],
        ),
        ("ReclaimPlan", &plan, &["unaccounted_bytes"][..]),
    ] {
        for field in fields {
            let mut unknown = value.clone();
            unknown[field] = Value::Null;
            assert!(schema.errors(def, &unknown).is_empty());
            assert_eq!(roundtrip(def, &unknown).unwrap().unwrap(), unknown);
            unknown.as_object_mut().unwrap().remove(*field);
            assert!(!schema.errors(def, &unknown).is_empty());
            assert!(roundtrip(def, &unknown).unwrap().is_err());
        }
        let mut extra = value.clone();
        extra["execute"] = json!(true);
        assert!(!schema.errors(def, &extra).is_empty());
        assert!(roundtrip(def, &extra).unwrap().is_err());
    }
    for limit in [0, 201] {
        let value = json!({"volume_id":"vo_lab1", "limit":limit});
        assert!(!schema.errors("ReclaimPlanRequest", &value).is_empty());
        assert!(roundtrip("ReclaimPlanRequest", &value).unwrap().is_err());
    }
    let request = json!({"volume_id": "vo_lab1", "limit": 50});
    assert!(schema.errors("ReclaimPlanRequest", &request).is_empty());
    assert_eq!(
        roundtrip("ReclaimPlanRequest", &request).unwrap().unwrap(),
        request
    );
    for volume_id in ["", "synthetic_volume", "rt_lab1"] {
        let invalid = json!({"volume_id": volume_id, "limit": 50});
        assert!(!schema.errors("ReclaimPlanRequest", &invalid).is_empty());
        assert!(roundtrip("ReclaimPlanRequest", &invalid).unwrap().is_err());
        let mut invalid_volume = plan["volume"].clone();
        invalid_volume["volume_id"] = json!(volume_id);
        assert!(!schema.errors("ReclaimVolume", &invalid_volume).is_empty());
        assert!(roundtrip("ReclaimVolume", &invalid_volume)
            .unwrap()
            .is_err());
    }
    let legacy = json!({"volume_key": "synthetic_volume", "limit": 50});
    assert!(!schema.errors("ReclaimPlanRequest", &legacy).is_empty());
    assert!(roundtrip("ReclaimPlanRequest", &legacy).unwrap().is_err());
    assert_eq!(plan["volume"]["volume_id"], "vo_lab1");
    assert_eq!(Command::ReclaimPlan.client_result_def(), None);
}

#[test]
fn reclaim_selection_is_additive_bounded_and_preserves_exact_bytes() {
    let mut schema = Schema::load();
    for (def, mut request) in [
        (
            "ReclaimPlanRequest",
            json!({"volume_id":"vo_lab1", "limit":50}),
        ),
        ("ReclaimObserveRequest", json!({"volume_id":"vo_lab1"})),
    ] {
        assert_eq!(roundtrip(def, &request).unwrap().unwrap(), request);
        for length in [0, 1, 12, 13] {
            request["selection"] = json!((0..length)
                .map(|i| format!("nd_case{i}"))
                .collect::<Vec<_>>());
            let valid = (1..=12).contains(&length);
            assert_eq!(schema.errors(def, &request).is_empty(), valid);
            assert_eq!(roundtrip(def, &request).unwrap().is_ok(), valid);
        }
        request["selection"] = Value::Null;
        assert!(!schema.errors(def, &request).is_empty());
        assert!(roundtrip(def, &request).unwrap().is_err());
    }
    let value = json!({"members":["nd_a","nd_b"], "status":"recorded",
        "observed_at":"2026-10-29T00:00:00Z", "reclaimable_bytes":"18446744073709551615", "reason":null});
    assert!(schema.errors("ReclaimSelection", &value).is_empty());
    assert_eq!(
        roundtrip("ReclaimSelection", &value).unwrap().unwrap(),
        value
    );
    for field in ["observed_at", "reclaimable_bytes", "reason"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(!schema.errors("ReclaimSelection", &missing).is_empty());
        assert!(roundtrip("ReclaimSelection", &missing).unwrap().is_err());
    }
}

#[test]
fn insight_folder_is_owner_only_closed_and_never_a_probability_or_approval() {
    let mut schema = Schema::load();
    let result = read(&contracts().join("examples/commands/insight.folder.result.json"));
    for (def, value, fields) in [
        ("InsightFolder", &result, &["generation"][..]),
        (
            "InsightAxis",
            &result["axes"][0],
            &["label", "abstain_reason"][..],
        ),
    ] {
        for field in fields {
            let mut unknown = value.clone();
            unknown[field] = Value::Null;
            assert!(schema.errors(def, &unknown).is_empty());
            assert_eq!(roundtrip(def, &unknown).unwrap().unwrap(), unknown);
            unknown.as_object_mut().unwrap().remove(*field);
            assert!(!schema.errors(def, &unknown).is_empty());
            assert!(roundtrip(def, &unknown).unwrap().is_err());
        }
    }
    // Closed everywhere: no probability, score, approval or effect field can ride along.
    for (def, value) in [
        ("InsightFolder", &result),
        ("InsightAxis", &result["axes"][0]),
        ("InsightVote", &result["axes"][0]["votes"][0]),
        ("InsightLine", &result["lines"][0]),
    ] {
        for extra in ["probability", "score", "approved", "execute"] {
            let mut padded = value.clone();
            padded[extra] = json!(true);
            assert!(!schema.errors(def, &padded).is_empty(), "{def} {extra}");
            assert!(roundtrip(def, &padded).unwrap().is_err(), "{def} {extra}");
        }
    }
    // Exactly the three axes.
    let mut two = result.clone();
    two["axes"].as_array_mut().unwrap().pop();
    assert!(!schema.errors("InsightFolder", &two).is_empty());
    assert!(roundtrip("InsightFolder", &two).unwrap().is_err());
    for request in [
        json!({"root_id":"rt_synthetic"}),
        json!({"node_id":"nd_synthetic"}),
        json!({"root_id":"rt_synthetic","node_id":"nd_synthetic","limit":5}),
    ] {
        assert!(!schema.errors("InsightFolderRequest", &request).is_empty());
        assert!(roundtrip("InsightFolderRequest", &request)
            .unwrap()
            .is_err());
    }
    assert_eq!(Command::InsightFolder.client_result_def(), None);
}

#[test]
fn changes_new_since_is_owner_only_closed_and_keeps_unknowns_null() {
    let mut schema = Schema::load();
    let result = read(&contracts().join("examples/commands/changes.new_since.result.json"));
    let request = read(&contracts().join("examples/commands/changes.new_since.request.json"));
    // Every unknown is a required null, never an omitted field and never a zero.
    for (def, value, fields) in [
        (
            "ChangesNewSince",
            &result,
            &[
                "generation",
                "baseline",
                "latest",
                "new_files",
                "new_dirs",
                "logical_bytes",
                "allocated_bytes",
                "allocation_unknown_files",
            ][..],
        ),
        ("NewSinceRun", &result["baseline"], &["finished_at"][..]),
        (
            "NewSinceFile",
            &result["largest_files"][0],
            &["allocated_bytes"][..],
        ),
        (
            "NewSinceFolder",
            &result["largest_folders"][0],
            &["logical_bytes", "allocated_bytes"][..],
        ),
    ] {
        for field in fields {
            let mut unknown = value.clone();
            unknown[field] = Value::Null;
            assert!(schema.errors(def, &unknown).is_empty(), "{def} {field}");
            assert_eq!(roundtrip(def, &unknown).unwrap().unwrap(), unknown);
            unknown.as_object_mut().unwrap().remove(*field);
            assert!(!schema.errors(def, &unknown).is_empty(), "{def} {field}");
            assert!(roundtrip(def, &unknown).unwrap().is_err(), "{def} {field}");
        }
    }
    // No earlier scan: the outcome says so and nothing is a number.
    let mut none = result.clone();
    none["outcome"] = json!("no_baseline");
    for field in [
        "baseline",
        "new_files",
        "new_dirs",
        "logical_bytes",
        "allocated_bytes",
        "allocation_unknown_files",
    ] {
        none[field] = Value::Null;
    }
    none["largest_files"] = json!([]);
    none["largest_folders"] = json!([]);
    none["partial_reasons"] = json!([]);
    none["caveats"] = json!([]);
    none["partial"] = json!(false);
    assert!(schema.errors("ChangesNewSince", &none).is_empty());
    assert_eq!(roundtrip("ChangesNewSince", &none).unwrap().unwrap(), none);
    // Closed everywhere: no probability, score, approval or effect field can ride along.
    for (def, value) in [
        ("ChangesNewSince", &result),
        ("NewSinceRun", &result["baseline"]),
        ("NewSinceFile", &result["largest_files"][0]),
        ("NewSinceFolder", &result["largest_folders"][0]),
        ("NewSinceCaveat", &result["caveats"][0]),
        ("ChangesNewSinceRequest", &request),
    ] {
        for extra in ["probability", "score", "approved", "execute"] {
            let mut padded = value.clone();
            padded[extra] = json!(true);
            assert!(!schema.errors(def, &padded).is_empty(), "{def} {extra}");
            assert!(roundtrip(def, &padded).unwrap().is_err(), "{def} {extra}");
        }
    }
    // The reasons and caveats are closed vocabularies; a byte count is a decimal string, a count an integer.
    let mut reason = result.clone();
    reason["partial_reasons"] = json!(["looks_fine"]);
    assert!(!schema.errors("ChangesNewSince", &reason).is_empty());
    assert!(roundtrip("ChangesNewSince", &reason).unwrap().is_err());
    let mut number = result.clone();
    number["logical_bytes"] = json!(5);
    assert!(!schema.errors("ChangesNewSince", &number).is_empty());
    assert!(roundtrip("ChangesNewSince", &number).unwrap().is_err());
    let mut text = result.clone();
    text["new_files"] = json!("340");
    assert!(!schema.errors("ChangesNewSince", &text).is_empty());
    assert!(roundtrip("ChangesNewSince", &text).unwrap().is_err());
    // The request: baseline_run is required (null is the default scan), the limit is 1..=50.
    let mut named = request.clone();
    named["baseline_run"] = json!("12");
    assert!(schema.errors("ChangesNewSinceRequest", &named).is_empty());
    assert_eq!(
        roundtrip("ChangesNewSinceRequest", &named)
            .unwrap()
            .unwrap(),
        named
    );
    for bad in [
        json!({"root_id":"rt_synthetic","limit":5}),
        json!({"root_id":"rt_synthetic","baseline_run":null,"limit":0}),
        json!({"root_id":"rt_synthetic","baseline_run":null,"limit":51}),
        json!({"root_id":"rt_synthetic","baseline_run":12,"limit":5}),
        json!({"root_id":"rt_synthetic","baseline_run":"-1","limit":5}),
        json!({"baseline_run":null,"limit":5}),
        json!({"root_id":"rt_synthetic","baseline_run":null,"limit":5,"cursor":null}),
    ] {
        assert!(
            !schema.errors("ChangesNewSinceRequest", &bad).is_empty(),
            "{bad}"
        );
        assert!(
            roundtrip("ChangesNewSinceRequest", &bad).unwrap().is_err(),
            "{bad}"
        );
    }
    assert_eq!(Command::ChangesNewSince.client_result_def(), None);
}

#[test]
fn reclaim_relief_keeps_unknowns_null_and_refuses_extra_fields() {
    let mut schema = Schema::load();
    let result = read(&contracts().join("examples/commands/reclaim.sample.result.json"));
    // Not compared: every unknown is null, never zero, and each field is still required.
    let mut unknown = result.clone();
    for field in ["after", "direction", "change_bytes", "capacity_changed"] {
        unknown[field] = Value::Null;
    }
    unknown["status"] = json!("not_checked");
    assert!(schema.errors("ReclaimRelief", &unknown).is_empty());
    assert_eq!(
        roundtrip("ReclaimRelief", &unknown).unwrap().unwrap(),
        unknown
    );
    for field in ["after", "direction", "change_bytes", "capacity_changed"] {
        let mut missing = unknown.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(!schema.errors("ReclaimRelief", &missing).is_empty());
        assert!(roundtrip("ReclaimRelief", &missing).unwrap().is_err());
    }
    // #1031: `compared` is tied to a direction, a change and an after sample; any other status to none of them.
    let mut with = |edit: &dyn Fn(&mut Value)| {
        let mut v = result.clone();
        edit(&mut v);
        schema.errors("ReclaimRelief", &v).is_empty()
    };
    assert!(with(&|_| {}));
    assert!(with(&|v| v["capacity_changed"] = json!(true)));
    assert!(with(&|v| v["capacity_changed"] = Value::Null));
    for field in ["after", "direction", "change_bytes"] {
        assert!(
            !with(&|v| v[field] = Value::Null),
            "compared without {field}"
        );
    }
    for status in ["different_volume", "unreadable"] {
        let other = |v: &mut Value| {
            v["status"] = json!(status);
            v["direction"] = Value::Null;
            v["change_bytes"] = Value::Null;
            v["capacity_changed"] = Value::Null;
        };
        assert!(with(&other), "{status}");
        assert!(
            !with(&|v| {
                other(v);
                v["after"] = Value::Null;
            }),
            "{status} needs the after sample that was refused"
        );
        for field in ["direction", "change_bytes", "capacity_changed"] {
            let stray = if field == "direction" {
                json!("more_free")
            } else if field == "change_bytes" {
                json!("1")
            } else {
                json!(false)
            };
            assert!(
                !with(&|v| {
                    other(v);
                    v[field] = stray.clone();
                }),
                "{status} with {field}"
            );
        }
    }
    assert!(
        !with(&|v| {
            v["status"] = json!("not_checked");
            v["direction"] = Value::Null;
            v["change_bytes"] = Value::Null;
            v["capacity_changed"] = Value::Null;
        }),
        "not_checked with an after sample"
    );
    for field in [
        "capacity_bytes",
        "free_bytes",
        "recycle_bin_bytes",
        "estimate_bytes",
    ] {
        let mut sample = result["after"].clone();
        sample[field] = Value::Null;
        assert!(schema.errors("ReliefSample", &sample).is_empty());
        assert_eq!(roundtrip("ReliefSample", &sample).unwrap().unwrap(), sample);
        sample.as_object_mut().unwrap().remove(field);
        assert!(!schema.errors("ReliefSample", &sample).is_empty());
        assert!(roundtrip("ReliefSample", &sample).unwrap().is_err());
    }
    let mut exact = result.clone();
    exact["change_bytes"] = json!("18446744073709551615");
    assert!(schema.errors("ReclaimRelief", &exact).is_empty());
    assert_eq!(roundtrip("ReclaimRelief", &exact).unwrap().unwrap(), exact);
    for bad in [json!("-1"), json!(5), json!("18446744073709551616")] {
        let mut invalid = result.clone();
        invalid["change_bytes"] = bad;
        assert!(roundtrip("ReclaimRelief", &invalid).unwrap().is_err());
    }
    let mut extra = result.clone();
    extra["cleanup_freed"] = json!("10000");
    assert!(!schema.errors("ReclaimRelief", &extra).is_empty());
    assert!(roundtrip("ReclaimRelief", &extra).unwrap().is_err());
    let request = read(&contracts().join("examples/commands/reclaim.sample.request.json"));
    for bad in [
        json!({"volume_id":"vo_lab1"}),
        json!({"volume_id":"vo_lab1","role":"during"}),
        json!({"volume_id":"vo_lab1","role":"after","estimate_bytes":null}),
        json!({"volume_id":"vo_lab1","role":"after","path":"C:\\"}),
    ] {
        assert!(!schema.errors("ReclaimSampleRequest", &bad).is_empty());
        assert!(roundtrip("ReclaimSampleRequest", &bad).unwrap().is_err());
    }
    assert!(schema.errors("ReclaimSampleRequest", &request).is_empty());
    assert_eq!(
        roundtrip("ReclaimSampleRequest", &request)
            .unwrap()
            .unwrap(),
        request
    );
    assert_eq!(Command::ReclaimSample.client_result_def(), None);
}

#[test]
fn coverage_report_keeps_unknowns_null_bounds_its_lists_and_is_owner_only() {
    let mut schema = Schema::load();
    let report = read(&contracts().join("examples/commands/coverage.result.result.json"));
    // A path the catalogue does not hold and a size nobody could read are null, never a guess.
    for (list, field) in [
        ("not_covered", "node_id"),
        ("unknown", "node_id"),
        ("unknown", "bytes"),
    ] {
        let mut value = report.clone();
        value[list][if list == "unknown" { 0 } else { 1 }][field] = Value::Null;
        assert!(
            schema.errors("CoverageReport", &value).is_empty(),
            "{list}.{field}"
        );
        assert_eq!(roundtrip("CoverageReport", &value).unwrap().unwrap(), value);
        value[list][if list == "unknown" { 0 } else { 1 }]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(!schema.errors("CoverageReport", &value).is_empty());
        assert!(roundtrip("CoverageReport", &value).unwrap().is_err());
    }
    for bad in [
        json!({"outcome": "safe_to_delete"}),
        json!({"reasons": ["cancelled"]}),
        json!({"reasons": ["budget", "budget"]}),
        json!({"bytes_read": 5}),
        json!({"totals": {"covered": {"files": 1, "bytes": "1"}}}),
        json!({"verdict": "covered"}),
    ] {
        let mut invalid = report.clone();
        for (key, value) in bad.as_object().unwrap() {
            invalid[key] = value.clone();
        }
        assert!(
            !schema.errors("CoverageReport", &invalid).is_empty(),
            "{bad}"
        );
        assert!(
            roundtrip("CoverageReport", &invalid).unwrap().is_err(),
            "{bad}"
        );
    }
    let mut full = report.clone();
    let item = report["not_covered"][0].clone();
    full["not_covered"] = Value::Array(vec![item.clone(); 200]);
    assert!(schema.errors("CoverageReport", &full).is_empty());
    assert_eq!(roundtrip("CoverageReport", &full).unwrap().unwrap(), full);
    full["not_covered"] = Value::Array(vec![item; 201]);
    assert!(!schema.errors("CoverageReport", &full).is_empty());
    assert!(roundtrip("CoverageReport", &full).unwrap().is_err());
    let request = read(&contracts().join("examples/commands/coverage.start.request.json"));
    for bad in [
        json!({"a_root_id": "rt_a"}),
        json!({"a_root_id": "rt_a", "b_root_id": "rt_b", "budget_secs": 0}),
        json!({"a_root_id": "rt_a", "b_root_id": "rt_b", "budget_secs": 3601}),
        json!({"a_root_id": "rt_a", "b_root_id": "rt_b", "budget_bytes": 5}),
        json!({"a_root_id": "rt_a", "b_root_id": "rt_b", "budget_bytes": null}),
        json!({"a_root_id": "rt_a", "b_root_id": "rt_b", "path": "C:\\"}),
    ] {
        assert!(
            !schema.errors("CoverageStartRequest", &bad).is_empty(),
            "{bad}"
        );
        assert!(
            roundtrip("CoverageStartRequest", &bad).unwrap().is_err(),
            "{bad}"
        );
    }
    assert!(schema.errors("CoverageStartRequest", &request).is_empty());
    assert_eq!(
        roundtrip("CoverageStartRequest", &request)
            .unwrap()
            .unwrap(),
        request
    );
    // Owner-only: no client projection exists for either command.
    assert_eq!(Command::CoverageStart.client_result_def(), None);
    assert_eq!(Command::CoverageResult.client_result_def(), None);
}

#[test]
fn diagnostics_elevated_session_refusal_roundtrips_through_schema_and_dto() {
    let value = json!({
        "outcome": "refused", "bytes_written": null, "refusal": "elevated_session"
    });
    let mut schema = Schema::load();
    assert!(schema.errors("DiagnosticsSaveResult", &value).is_empty());
    assert_eq!(
        roundtrip("DiagnosticsSaveResult", &value).unwrap().unwrap(),
        value
    );
}

#[test]
fn an_unsettled_search_row_has_required_nullable_sizes() {
    let mut row = read(&contracts().join("examples/commands/search.query.result.json"))["hits"][0]
        ["row"]
        .clone();
    row["kind"] = json!("dir");
    row["coverage"] = json!("unknown");
    for field in ["logical_bytes", "allocated_bytes", "files", "dirs"] {
        row[field] = Value::Null;
    }
    let mut schema = Schema::load();
    assert!(schema.errors("EntryRow", &row).is_empty());
    assert_eq!(roundtrip("EntryRow", &row).unwrap().unwrap(), row);
    row.as_object_mut().unwrap().remove("logical_bytes");
    assert!(!schema.errors("EntryRow", &row).is_empty());
    assert!(roundtrip("EntryRow", &row).unwrap().is_err());
}

fn read(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

struct Schema {
    root: Value,
    validators: HashMap<String, Validator>,
}

#[test]
fn client_projection_contracts_preserve_omitted_and_unknown_fields() {
    let mut schema = Schema::load();
    let owner = read(&contracts().join("examples/commands/node.inspect.result.json"));
    let owner_page = read(&contracts().join("examples/commands/search.query.result.json"));
    for mask in 0..16 {
        let mut row = owner["row"].clone();
        for (i, fields) in [
            &["location_hint"][..],
            &["logical_bytes", "allocated_bytes", "files", "dirs"][..],
            &["modified_at"][..],
            &["attributes", "flags"][..],
        ]
        .iter()
        .enumerate()
        {
            if mask & (1 << i) == 0 {
                for field in *fields {
                    row.as_object_mut().unwrap().remove(*field);
                }
            }
        }
        let mut node = json!({"row": row, "root_id": owner["root_id"]});
        let mut hit = owner_page["hits"][0].clone();
        hit["row"] = row.clone();
        if mask & 1 != 0 {
            node["display_path"] = owner["display_path"].clone();
        } else {
            hit.as_object_mut().unwrap().remove("path");
        }
        let mut page = owner_page.clone();
        page["hits"] = json!([hit]);
        for (def, value) in [
            ("ClientEntryRow", &row),
            ("ClientSearchHit", &hit),
            ("ClientNodeDetail", &node),
            ("ClientSearchPage", &page),
        ] {
            assert!(schema.errors(def, value).is_empty(), "{def}: {value}");
            assert_eq!(roundtrip(def, value).unwrap().unwrap(), *value);
            let mut invalid = value.clone();
            invalid["owner_only"] = json!(true);
            assert!(!schema.errors(def, &invalid).is_empty());
            assert!(roundtrip(def, &invalid).unwrap().is_err());
        }
        for (command, value) in [(Command::NodeInspect, node), (Command::SearchQuery, page)] {
            command
                .validate_client_result(value.as_object().unwrap())
                .unwrap();
            let mut invalid = value.clone();
            let row = if command == Command::NodeInspect {
                &mut invalid["row"]
            } else {
                &mut invalid["hits"][0]["row"]
            };
            row.as_object_mut().unwrap().remove("node_id");
            assert!(!schema
                .errors(command.client_result_def().unwrap(), &invalid)
                .is_empty());
            assert!(command
                .validate_client_result(invalid.as_object().unwrap())
                .is_err());
        }
    }
    // Optional disclosure fields keep their value constraints when present.
    for (field, value) in [
        ("attributes", Value::Null),
        ("flags", Value::Null),
        ("logical_bytes", json!(-1)),
        ("modified_at", json!("yesterday")),
    ] {
        let mut row = owner["row"].clone();
        row[field] = value;
        assert!(!schema.errors("ClientEntryRow", &row).is_empty());
        assert!(roundtrip("ClientEntryRow", &row).unwrap().is_err());
    }
    assert!(Command::HealthGet
        .validate_client_result(&Default::default())
        .is_err());
}

#[test]
fn client_grant_contract_accepts_all_five_disclosures_including_volumes() {
    let mut schema = Schema::load();
    let value = json!({"roots": ["rt_synthetic"], "fields": ["path", "size", "modified", "attributes", "volumes"]});
    assert!(schema.errors("AskGrantRequest", &value).is_empty());
    assert_eq!(
        roundtrip("AskGrantRequest", &value).unwrap().unwrap(),
        value
    );
    let grant = json!({"roots": value["roots"], "fields": value["fields"], "granted_at": "2026-10-25T00:00:00.000Z"});
    assert!(schema.errors("AskGrant", &grant).is_empty());
    assert_eq!(roundtrip("AskGrant", &grant).unwrap().unwrap(), grant);
    for fields in [json!(["volumes", "volumes"]), json!(["execute"])] {
        let invalid = json!({"roots": ["rt_synthetic"], "fields": fields});
        assert!(!schema.errors("AskGrantRequest", &invalid).is_empty());
        assert!(roundtrip("AskGrantRequest", &invalid).unwrap().is_err());
    }
}

impl Schema {
    fn load() -> Self {
        Self {
            root: read(&contracts().join("view-service.schema.json")),
            validators: HashMap::new(),
        }
    }

    fn defs(&self) -> &serde_json::Map<String, Value> {
        self.root["$defs"].as_object().unwrap()
    }

    /// Validates `value` against one `$def` with format checks on (date-time).
    fn errors(&mut self, def: &str, value: &Value) -> Vec<String> {
        if !self.validators.contains_key(def) {
            assert!(self.defs().contains_key(def), "no $def named {def}");
            let mut schema = self.root.clone();
            let obj = schema.as_object_mut().unwrap();
            obj.remove("anyOf");
            obj.insert("$ref".into(), json!(format!("#/$defs/{def}")));
            let validator = jsonschema::draft202012::options()
                .should_validate_formats(true)
                .build(&schema)
                .unwrap_or_else(|e| panic!("{def}: {e}"));
            self.validators.insert(def.into(), validator);
        }
        self.validators[def]
            .iter_errors(value)
            .map(|e| e.to_string())
            .collect()
    }
}

impl Schema {
    /// Schema errors for a whole envelope, with its payload checked against the type
    /// `commands.json` names for the envelope's command or event (semantics.md section 10).
    /// `command` pairs a response with the request it answers.
    fn complete_errors(
        &mut self,
        def: &str,
        value: &Value,
        command: Option<Command>,
    ) -> Vec<String> {
        let mut errors = self.errors(def, value);
        let (payload, payload_def) = match def {
            "RequestEnvelope" => {
                let cmd = value["command"].as_str().and_then(Command::from_name);
                (&value["payload"], cmd.map(|c| c.request_def()))
            }
            "ResponseEnvelope" if value["ok"] == json!(true) => {
                (&value["result"], command.map(|c| c.result_def()))
            }
            "EventEnvelope" => {
                let ev = serde_json::from_value::<EventName>(value["event"].clone()).ok();
                (&value["data"], ev.map(|e| e.data_def()))
            }
            _ => return errors,
        };
        match payload_def {
            Some(d) => errors.extend(self.errors(d, payload)),
            None => errors.push("no payload type for this envelope".into()),
        }
        errors
    }
}

fn files(dir: &str) -> Vec<PathBuf> {
    let mut out: Vec<_> = fs::read_dir(contracts().join("examples").join(dir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    out.sort();
    out
}

fn stem(p: &Path) -> String {
    p.file_name().unwrap().to_str().unwrap().to_string()
}

/// Every valid example in one place: (file label, def, value).
fn valid_examples() -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    for c in Command::ALL {
        for (kind, def) in [("request", c.request_def()), ("result", c.result_def())] {
            let p = contracts().join(format!("examples/commands/{}.{kind}.json", c.as_str()));
            out.push((stem(&p), def.to_string(), read(&p)));
        }
    }
    for e in EventName::ALL {
        let p = contracts().join(format!("examples/events/{}.json", e.as_str()));
        out.push((stem(&p), e.data_def().to_string(), read(&p)));
    }
    for dir in ["envelopes", "dtos"] {
        for p in files(dir) {
            let doc = read(&p);
            out.push((
                stem(&p),
                doc["def"].as_str().unwrap().to_string(),
                doc["value"].clone(),
            ));
        }
    }
    out
}

#[test]
fn example_directories_hold_exactly_the_expected_files() {
    let expect_commands: BTreeSet<_> = Command::ALL
        .iter()
        .flat_map(|c| {
            [
                format!("{}.request.json", c.as_str()),
                format!("{}.result.json", c.as_str()),
            ]
        })
        .collect();
    let have: BTreeSet<_> = files("commands").iter().map(|p| stem(p)).collect();
    assert_eq!(
        have, expect_commands,
        "one request and one result example per command"
    );
    let expect_events: BTreeSet<_> = EventName::ALL
        .iter()
        .map(|e| format!("{}.json", e.as_str()))
        .collect();
    let have: BTreeSet<_> = files("events").iter().map(|p| stem(p)).collect();
    assert_eq!(have, expect_events, "one example per event");
    let have: BTreeSet<_> = files("dtos").iter().map(|p| stem(p)).collect();
    assert_eq!(
        have,
        BTreeSet::from([
            "ClientField.json".to_string(),
            "McpClient.json".to_string(),
            "copy-disposition.json".to_string(),
            "copy-receipt.json".to_string(),
            "copy-receipt-published-unverified.json".to_string(),
            "disclosure-summary.json".to_string(),
            "duplicates-result.json".to_string(),
            "identity-observation-unverified.json".to_string(),
            "reclaim-observe-running.json".to_string(),
            "reclaim-selection-request.json".to_string(),
            "reclaim-selection-recorded.json".to_string(),
            "search-freshness-restarting.json".to_string(),
            "search-page-federated.json".to_string(),
            "search-request-federated.json".to_string()
        ])
    );
    assert!(
        files("invalid").len() >= 40,
        "the negatives are the point of the suite"
    );
}

#[test]
fn job_counters_are_required_unsigned_exact_integers() {
    let mut schema = Schema::load();
    let job = read(&contracts().join("examples/commands/jobs.get.result.json"))["job"].clone();
    for field in ["seq", "rev"] {
        for valid in [0, 9_007_199_254_740_991_u64] {
            let mut value = job.clone();
            value[field] = json!(valid);
            assert!(schema.errors("Job", &value).is_empty());
            assert_eq!(roundtrip("Job", &value).unwrap().unwrap(), value);
            assert!(serde_json::from_value::<loomward_protocol::Job>(value).is_ok());
        }
        let mut missing = job.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(!schema.errors("Job", &missing).is_empty());
        assert!(roundtrip("Job", &missing).unwrap().is_err());
        assert!(serde_json::from_value::<loomward_protocol::Job>(missing).is_err());
        for invalid in [
            json!(-1),
            json!(1.5),
            json!(null),
            json!("1"),
            json!(9_007_199_254_740_992_u64),
        ] {
            let mut value = job.clone();
            value[field] = invalid;
            assert!(!schema.errors("Job", &value).is_empty());
            assert!(roundtrip("Job", &value).unwrap().is_err());
            assert!(serde_json::from_value::<loomward_protocol::Job>(value).is_err());
        }
    }
}

#[test]
fn every_def_has_a_rust_type() {
    let schema = Schema::load();
    for def in schema.defs().keys() {
        assert!(
            roundtrip(def, &Value::Null).is_some(),
            "no Rust type registered for $def {def}"
        );
    }
}

#[test]
fn every_enum_value_round_trips() {
    let schema = Schema::load();
    for (def, node) in schema.defs() {
        let Some(values) = node.get("enum").and_then(Value::as_array) else {
            continue;
        };
        for v in values {
            let got = roundtrip(def, v)
                .unwrap()
                .unwrap_or_else(|e| panic!("{def} {v}: {e}"));
            assert_eq!(&got, v, "{def}");
        }
    }
}

#[test]
fn valid_examples_validate_and_round_trip() {
    let mut schema = Schema::load();
    let examples = valid_examples();
    assert_eq!(
        examples.len(),
        Command::ALL.len() * 2
            + EventName::ALL.len()
            + files("envelopes").len()
            + files("dtos").len()
    );
    for (label, def, value) in examples {
        let errors = schema.errors(&def, &value);
        assert!(
            errors.is_empty(),
            "{label} is not a valid {def}: {errors:?}"
        );
        let back = roundtrip(&def, &value)
            .unwrap_or_else(|| panic!("{label}: no Rust type for {def}"))
            .unwrap_or_else(|e| panic!("{label} does not deserialise as {def}: {e}"));
        assert_eq!(
            back, value,
            "{label} changed on the round trip through {def}"
        );
    }
}

/// The Rust verdict for a whole value, including the complete-envelope rule for envelopes.
fn rust_verdict(def: &str, value: &Value, complete: bool) -> Result<(), String> {
    roundtrip(def, value)
        .unwrap_or_else(|| panic!("no Rust type for {def}"))
        .map_err(|e| e.to_string())?;
    if !complete {
        return Ok(());
    }
    match def {
        "EventEnvelope" => decode_exact::<EventEnvelope>(value.clone())
            .and_then(|e| e.validate())
            .map_err(|e| e.message.to_string()),
        "RequestEnvelope" => decode_exact::<RequestEnvelope>(value.clone())
            .and_then(|e| e.validate().map(drop))
            .map_err(|e| e.message.to_string()),
        _ => Ok(()),
    }
}

#[test]
fn invalid_examples_fail_the_schema_and_the_dto() {
    let mut schema = Schema::load();
    let (mut rust_only, mut complete) = (0, 0);
    for p in files("invalid") {
        let doc = read(&p);
        let (def, value) = (doc["def"].as_str().unwrap(), &doc["value"]);
        let flag = |k: &str| doc.get(k) == Some(&json!(true));
        assert!(
            doc["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{} needs a reason",
            stem(&p)
        );
        let schema_errors = if flag("complete") {
            schema.complete_errors(def, value, None)
        } else {
            schema.errors(def, value)
        };
        if flag("rust_only") {
            // JSON Schema cannot express this rule; the runtime check must.
            rust_only += 1;
            assert!(
                schema_errors.is_empty(),
                "{} is marked rust_only but the schema rejects it",
                stem(&p)
            );
        } else {
            assert!(
                !schema_errors.is_empty(),
                "{} passes the schema as {def}",
                stem(&p)
            );
        }
        complete += usize::from(flag("complete"));
        assert!(
            rust_verdict(def, value, flag("complete")).is_err(),
            "{} deserialises as {def} but must not",
            stem(&p)
        );
    }
    assert!(rust_only >= 2 && complete >= 3, "{rust_only} {complete}");
}

/// Complete envelopes: every command's request and ok response and every event, wrapped in a
/// full envelope, validate with their payload discriminated through commands.json (schema
/// and Rust), carry revisions, and round-trip.
#[test]
fn complete_envelopes_validate_for_every_command_and_event() {
    let mut schema = Schema::load();
    let meta = json!({"served_at":"2026-10-09T12:00:00.250Z","elapsed_ms":3,"dataset_class":"synthetic","budget_hit":false,"catalog_rev":"8812","state_rev":"311"});
    for c in Command::ALL {
        let payload =
            read(&contracts().join(format!("examples/commands/{}.request.json", c.as_str())));
        let result =
            read(&contracts().join(format!("examples/commands/{}.result.json", c.as_str())));
        let request = json!({"protocol":"loomward/3","request_id":"r_1","command":c.as_str(),"payload":payload});
        let errs = schema.complete_errors("RequestEnvelope", &request, None);
        assert!(errs.is_empty(), "{} request: {errs:?}", c.as_str());
        let env = decode_exact::<RequestEnvelope>(request.clone()).unwrap();
        assert_eq!(env.validate().unwrap(), *c);
        assert_eq!(serde_json::to_value(&env).unwrap(), request);

        let response = json!({"protocol":"loomward/3","request_id":"r_1","ok":true,"result":result,"meta":meta});
        let errs = schema.complete_errors("ResponseEnvelope", &response, Some(*c));
        assert!(errs.is_empty(), "{} response: {errs:?}", c.as_str());
        match decode_exact::<ResponseEnvelope>(response.clone()).unwrap() {
            ResponseEnvelope::Ok(ok) => ok.validate_for(*c).unwrap(),
            ResponseEnvelope::Err(_) => panic!("ok response decoded as an error"),
        }
    }
    for e in EventName::ALL {
        let data = read(&contracts().join(format!("examples/events/{}.json", e.as_str())));
        let event = json!({"protocol":"loomward/3","epoch":"e_k3J9x2Qa","seq":9,"event":e.as_str(),"at":"2026-10-09T12:00:00.250Z","catalog_rev":null,"state_rev":null,"data":data});
        let errs = schema.complete_errors("EventEnvelope", &event, None);
        assert!(errs.is_empty(), "{} event: {errs:?}", e.as_str());
        let env = decode_exact::<EventEnvelope>(event.clone()).unwrap();
        env.validate().unwrap();
        assert_eq!(serde_json::to_value(&env).unwrap(), event);
    }
}

/// Every object with declared properties is closed.
#[test]
fn objects_with_properties_are_closed() {
    let schema = Schema::load();
    fn walk(path: &str, node: &Value, open: &mut Vec<String>) {
        match node {
            Value::Object(map) => {
                if map.contains_key("properties")
                    && map.get("additionalProperties") != Some(&json!(false))
                {
                    open.push(path.to_string());
                }
                for (k, v) in map {
                    walk(&format!("{path}/{k}"), v, open);
                }
            }
            Value::Array(items) => items
                .iter()
                .enumerate()
                .for_each(|(i, v)| walk(&format!("{path}/{i}"), v, open)),
            _ => {}
        }
    }
    let mut open = Vec::new();
    walk("", &schema.root["$defs"], &mut open);
    assert!(open.is_empty(), "open objects: {open:?}");
}

#[test]
fn every_effect_capability_is_the_constant_false() {
    let schema = Schema::load();
    let effects = &schema.defs()["Capabilities"]["properties"]["effects"];
    let props = effects["properties"].as_object().unwrap();
    assert_eq!(props.len(), 11);
    assert_eq!(effects["required"].as_array().unwrap().len(), props.len());
    for (name, node) in props {
        assert_eq!(node, &json!({"const": false}), "effects.{name}");
    }
}

/// No request carries a path, command line or executable name (invariants 1 and 2): checked
/// on property names across every request def and everything it references.
#[test]
fn no_request_accepts_a_path() {
    let schema = Schema::load();
    const FORBIDDEN: &[&str] = &[
        "path",
        "paths",
        "file",
        "files",
        "file_path",
        "dir",
        "directory",
        "folder",
        "filename",
        "cmdline",
        "command_line",
        "exe",
        "executable",
        "argv",
    ];
    fn props(schema: &Schema, node: &Value, seen: &mut BTreeSet<String>, hits: &mut Vec<String>) {
        match node {
            Value::Object(map) => {
                if let Some(r) = map.get("$ref").and_then(Value::as_str) {
                    let name = r.rsplit('/').next().unwrap().to_string();
                    if seen.insert(name.clone()) {
                        props(schema, &schema.defs()[&name], seen, hits);
                    }
                }
                if let Some(p) = map.get("properties").and_then(Value::as_object) {
                    hits.extend(
                        p.keys()
                            .filter(|k| FORBIDDEN.contains(&k.as_str()))
                            .cloned(),
                    );
                }
                map.values().for_each(|v| props(schema, v, seen, hits));
            }
            Value::Array(items) => items.iter().for_each(|v| props(schema, v, seen, hits)),
            _ => {}
        }
    }
    for c in Command::ALL {
        let mut hits = Vec::new();
        props(
            &schema,
            &schema.defs()[c.request_def()],
            &mut BTreeSet::new(),
            &mut hits,
        );
        assert!(
            hits.is_empty(),
            "{} request has path-like properties {hits:?}",
            c.as_str()
        );
    }
}

/// JSON pointers of every node, root included.
fn pointers(v: &Value, here: &str, out: &mut Vec<String>) {
    out.push(here.to_string());
    match v {
        Value::Object(m) => m.iter().for_each(|(k, c)| {
            pointers(
                c,
                &format!("{here}/{}", k.replace('~', "~0").replace('/', "~1")),
                out,
            )
        }),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .for_each(|(i, c)| pointers(c, &format!("{here}/{i}"), out)),
        _ => {}
    }
}

/// One-step corruptions of a valid value: drop a member, null it, retype it, add an unknown field.
fn mutants(v: &Value) -> Vec<(String, Value)> {
    let mut ptrs = Vec::new();
    pointers(v, "", &mut ptrs);
    let mut out = Vec::new();
    for p in &ptrs {
        let node = v.pointer(p).unwrap();
        if node.is_object() {
            let mut m = v.clone();
            m.pointer_mut(p)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("zz_extra".into(), json!(1));
            out.push((format!("{p} + unknown field"), m));
        }
        if p.is_empty() {
            continue;
        }
        let (parent, last) = p.rsplit_once('/').unwrap();
        let mut dropped = v.clone();
        match dropped.pointer_mut(parent).unwrap() {
            Value::Object(m) => {
                m.remove(&last.replace("~1", "/").replace("~0", "~"));
            }
            Value::Array(a) => {
                a.remove(last.parse::<usize>().unwrap());
            }
            _ => unreachable!(),
        }
        out.push((format!("{p} dropped"), dropped));
        for (label, repl) in [
            ("null", Value::Null),
            ("number", json!(7)),
            ("string", json!("x")),
            ("true", json!(true)),
            ("empty array", json!([])),
        ] {
            let mut m = v.clone();
            *m.pointer_mut(p).unwrap() = repl;
            out.push((format!("{p} -> {label}"), m));
        }
    }
    out
}

/// Cross-field rules JSON Schema cannot state (the `rust_only` class of invalid examples), checked
/// here directly on the JSON rather than through the DTO: true when `value` breaks one of them.
/// Keyed by the def the Rust DTO is decoded as. Only DTOs that embed these types are listed:
/// a `Job` carries `duplicates` (`JobResult.job`, each `JobList.jobs` item). A `ResponseEnvelope`
/// `result` is an untyped payload that the envelope roundtrip does not decode, so it is not checked.
fn cross_field_refusal(def: &str, value: &Value) -> bool {
    match def {
        "TreeSlice" => parents_break_order(&value["nodes"]),
        "DuplicateReport" => report_dangles(value),
        "CollectionMembersUpdate" => add_and_remove_overlap(value),
        "JobResult" => report_in_job(&value["job"]),
        "JobList" => value["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(report_in_job),
        _ => false,
    }
}

fn report_in_job(job: &Value) -> bool {
    job["duplicates"].is_object() && report_dangles(&job["duplicates"])
}

/// SliceNode.parent is null only at index 0, and every other parent names an earlier node.
fn parents_break_order(nodes: &Value) -> bool {
    nodes
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .any(|(index, node)| {
            let parent = &node["parent"];
            if index == 0 {
                !parent.is_null()
            } else {
                parent.as_u64().is_none_or(|p| p >= index as u64)
            }
        })
}

/// Group, link and skipped indices refer to entries: each must be below `entries.len()`.
fn report_dangles(report: &Value) -> bool {
    let count = report["entries"].as_array().map_or(0, Vec::len) as u64;
    let dangling = |index: &Value| index.as_u64().is_none_or(|i| i >= count);
    let sets = |key: &str| {
        report[key]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|set| set.as_array().into_iter().flatten())
    };
    sets("groups").chain(sets("links")).any(dangling)
        || report["skipped"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|skip| dangling(&skip["entry"]))
        // entries_compared counts listed entries, so it cannot exceed them.
        || report["entries_compared"].as_u64().is_some_and(|c| c > count)
}

/// A node may not be both added and removed in one update (semantics.md, Collections).
fn add_and_remove_overlap(update: &Value) -> bool {
    let members = |key: &str| update[key].as_array().cloned().unwrap_or_default();
    let remove = members("remove");
    members("add").iter().any(|id| remove.contains(id))
}

/// The Rust DTOs accept exactly what the schema accepts, less the cross-field refusals above, for
/// every one-step corruption of every example: a gap either way is a contract bug (a schema
/// loophole or a Rust loophole). A cross-field violation must be refused by Rust, never accepted.
#[test]
fn schema_and_dto_agree_on_corrupted_examples() {
    let mut schema = Schema::load();
    let mut checked = 0;
    let mut disagreements = Vec::new();
    for (label, def, value) in valid_examples() {
        for (what, mutant) in mutants(&value) {
            let schema_ok = schema.errors(&def, &mutant).is_empty();
            let rust_ok = roundtrip(&def, &mutant).unwrap().is_ok();
            let expected_rust_ok = schema_ok && !cross_field_refusal(&def, &mutant);
            checked += 1;
            if rust_ok != expected_rust_ok {
                disagreements.push(format!(
                    "{label} {what}: schema {schema_ok}, rust {rust_ok}"
                ));
            }
        }
    }
    assert!(checked > 5000, "only {checked} mutants");
    assert!(
        disagreements.is_empty(),
        "{} disagreements, first 25:\n{}",
        disagreements.len(),
        disagreements
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
