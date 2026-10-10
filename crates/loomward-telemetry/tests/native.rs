use loomward_telemetry::{snapshot, Observation};
use std::process::Command;
#[cfg(windows)]
use {loomward_telemetry::Sampler, std::time::Duration};

#[test]
fn snapshot_json_has_documented_fields() {
    let output = Command::new(env!("CARGO_BIN_EXE_loomward-telemetry"))
        .args(["snapshot", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for field in [
        "schema_version",
        "status",
        "memory",
        "processes",
        "process_totals",
        "gpu",
        "process_gpu",
        "disk_io",
        "snapshot_cost_ms",
        "sample_interval_seconds",
    ] {
        assert!(value.get(field).is_some(), "missing {field}");
    }
    assert!(value["sample_interval_seconds"].is_null());
    assert_eq!(value["schema_version"], 2);
    if value["memory"]["status"] == "observed" {
        for field in [
            "total_bytes",
            "available_bytes",
            "commit_bytes",
            "standby_bytes",
            "modified_bytes",
            "free_bytes",
            "load_fraction",
        ] {
            assert!(
                value["memory"]["value"].get(field).is_some(),
                "missing {field}"
            );
        }
        assert!(value["memory"]["value"]
            .get("physical_total_bytes")
            .is_none());
    }
    for process in value["processes"].as_array().unwrap() {
        assert!(process.get("private_commit_bytes").is_some());
        assert!(process.get("private_working_set_bytes").is_some());
        assert!(process.get("private_bytes").is_none());
        assert!(process["rates"].get("cpu_fraction").is_some());
        assert!(process["rates"].get("cpu_percent_of_machine").is_none());
    }
    assert!(serde_json::from_value::<loomward_telemetry::Snapshot>(value).is_ok());
}

#[test]
fn watch_is_ndjson_and_first_rates_are_unknown() {
    let output = Command::new(env!("CARGO_BIN_EXE_loomward-telemetry"))
        .args(["watch", "--interval-ms", "100", "--count", "2", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    let rows: Vec<loomward_telemetry::Snapshot> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].sample_interval_seconds.is_none());
    assert!(rows[1].sample_interval_seconds.unwrap() >= 0.1);
    assert!(rows[0]
        .processes
        .iter()
        .all(|p| p.rates.cpu_fraction.is_none()));
}

#[test]
fn malformed_cli_is_rejected() {
    for args in [
        vec!["snapshot", "--kill", "4"],
        vec!["watch", "--count", "0", "--json"],
        vec!["watch", "--interval-ms", "0", "--json"],
        vec!["watch", "--count", "18446744073709551615", "--json"],
    ] {
        assert!(!Command::new(env!("CARGO_BIN_EXE_loomward-telemetry"))
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
}

#[test]
fn display_is_bounded_and_totals_precede_projection() {
    let all = snapshot(usize::MAX);
    assert!(all.processes.len() <= loomward_telemetry::MAX_PROCESS_ROWS);
    let none = snapshot(0);
    assert!(none.processes.is_empty());
    assert_eq!(none.display_truncated, none.process_totals.enumerated > 0);
}

#[cfg(windows)]
#[test]
fn windows_observations_and_rates_have_structural_truths() {
    let mut sampler = Sampler::default();
    let first = sampler.sample(1024);
    let Observation::Observed { value: memory } = &first.memory else {
        panic!("memory not observed")
    };
    assert!(memory.total_bytes.unwrap() > 0);
    assert!(memory.available_bytes.unwrap() <= memory.total_bytes.unwrap());
    for (field, bytes) in [
        ("standby_bytes", memory.standby_bytes),
        ("modified_bytes", memory.modified_bytes),
        ("free_bytes", memory.free_bytes),
    ] {
        if let Some(bytes) = bytes {
            assert!(bytes <= memory.total_bytes.unwrap());
        } else {
            assert!(memory
                .unknowns
                .iter()
                .any(|u| u.field == field || u.field == "memory_lists"));
        }
    }
    let own = first
        .processes
        .iter()
        .find(|p| p.pid == std::process::id())
        .unwrap();
    assert!(own.working_set_bytes.unwrap() > 0);
    assert!(own.start_time_windows_100ns.is_some());
    assert!(own.rates.cpu_fraction.is_none());
    // PID 4 (System): what is readable depends on the token (hosted CI runners are elevated). Either
    // way an unread field is null with a recorded reason, never a silent zero.
    let system = first.processes.iter().find(|p| p.pid == 4).unwrap();
    if system.image_name.is_none() || system.working_set_bytes.is_none() {
        assert!(system.protected_or_unknown);
        assert!(!system.unknowns.is_empty());
    }
    assert_ne!(system.working_set_bytes, Some(0));
    std::thread::sleep(Duration::from_millis(100));
    let second = sampler.sample(1024);
    let own = second
        .processes
        .iter()
        .find(|p| p.pid == std::process::id())
        .unwrap();
    assert!((0.0..=1.0).contains(&own.rates.cpu_fraction.unwrap()));
    assert!(own.rates.io_read_bytes_per_s.unwrap() >= 0.0);
    assert!(second.sample_interval_seconds.unwrap() >= 0.1);
    match &second.gpu {
        Observation::Observed { value } => {
            assert!(!value.is_empty());
            let ids: std::collections::HashSet<_> = value.iter().map(|a| &a.adapter_id).collect();
            assert_eq!(ids.len(), value.len());
            for adapter in value {
                assert!(adapter.adapter_id.starts_with("luid_"));
                for (field, bytes) in [
                    ("dedicated_total_bytes", adapter.dedicated_total_bytes),
                    ("dedicated_used_bytes", adapter.dedicated_used_bytes),
                    ("shared_used_bytes", adapter.shared_used_bytes),
                ] {
                    if bytes.is_none() {
                        assert!(adapter.unknowns.iter().any(|u| u.field == field));
                    }
                }
                if let Some(fraction) = adapter.engine_busy_fraction {
                    assert!((0.0..=1.0).contains(&fraction));
                    assert_eq!(
                        Some(fraction),
                        adapter
                            .engines
                            .iter()
                            .filter_map(|e| e.busy_fraction)
                            .reduce(f64::max)
                    );
                } else {
                    assert!(adapter
                        .unknowns
                        .iter()
                        .any(|u| u.field == "engine_busy_fraction"));
                }
                for engine in &adapter.engines {
                    if let Some(fraction) = engine.busy_fraction {
                        assert!((0.0..=1.0).contains(&fraction));
                    } else {
                        assert!(!engine.unknowns.is_empty());
                    }
                }
            }
        }
        Observation::Unknown { reason } => assert!(!reason.is_empty()),
        Observation::Unsupported { .. } => panic!("Windows GPU query should be attempted"),
    }
    match second.disk_io {
        Observation::Observed { value } => {
            assert!(!value.is_empty());
            for disk in value {
                assert!(disk.sample_interval_seconds.unwrap() > 0.0);
                for (field, value) in [
                    ("read_bytes_per_s", disk.read_bytes_per_s),
                    ("write_bytes_per_s", disk.write_bytes_per_s),
                    ("busy_fraction", disk.busy_fraction),
                    ("queue_length", disk.queue_length),
                ] {
                    if let Some(value) = value {
                        assert!(value.is_finite() && value >= 0.0);
                        if field == "busy_fraction" {
                            assert!(value <= 1.0);
                        }
                    } else {
                        assert!(disk.unknowns.iter().any(|u| u.field == field));
                    }
                }
            }
        }
        Observation::Unknown { reason } => assert!(!reason.is_empty()),
        Observation::Unsupported { .. } => panic!("Windows disk query should be attempted"),
    }
}

#[cfg(windows)]
#[test]
fn rates_survive_a_process_not_being_displayed_in_the_first_sample() {
    let mut sampler = Sampler::default();
    assert!(sampler.sample(0).processes.is_empty());
    let own = sampler.own_process().unwrap();
    assert_eq!(own.pid, std::process::id());
    assert!(own.private_commit_bytes.unwrap() > 0);
    std::thread::sleep(Duration::from_millis(100));
    let next = sampler.sample(1024);
    let own = next
        .processes
        .iter()
        .find(|p| p.pid == std::process::id())
        .unwrap();
    assert!(own.rates.cpu_fraction.is_some());
}

#[test]
fn own_usage_has_structural_truth_or_an_explicit_unsupported_reason() {
    match loomward_telemetry::own_usage() {
        Observation::Observed { value } => {
            assert!(value.cpu_seconds >= 0.0);
            assert!(value.calling_thread_cpu_seconds >= 0.0);
            assert!(value.calling_thread_cpu_seconds <= value.cpu_seconds);
            assert!(value.private_commit_bytes > 0);
            assert!(value.peak_private_commit_bytes >= value.private_commit_bytes);
        }
        Observation::Unsupported { reason } => {
            #[cfg(windows)]
            panic!("Windows own usage should be attempted: {reason}");
            #[cfg(not(windows))]
            assert!(!reason.is_empty());
        }
        Observation::Unknown { reason } => panic!("own usage unavailable: {reason}"),
    }
}

#[cfg(not(windows))]
#[test]
fn non_windows_is_explicitly_unsupported() {
    assert_eq!(snapshot(200).status, "unsupported");
    assert!(matches!(
        snapshot(200).memory,
        Observation::Unsupported { .. }
    ));
}
