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
        .all(|p| p.rates.cpu_percent_of_machine.is_none()));
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
    assert!(memory.physical_total_bytes.unwrap() > 0);
    assert!(memory.physical_available_bytes.unwrap() <= memory.physical_total_bytes.unwrap());
    let own = first
        .processes
        .iter()
        .find(|p| p.pid == std::process::id())
        .unwrap();
    assert!(own.working_set_bytes.unwrap() > 0);
    assert!(own.start_time_windows_100ns.is_some());
    assert!(own.rates.cpu_percent_of_machine.is_none());
    let system = first.processes.iter().find(|p| p.pid == 4).unwrap();
    assert!(system.protected_or_unknown);
    assert!(system.image_name.is_none());
    assert!(system.working_set_bytes.is_none());
    assert!(system.unknowns.iter().any(|u| u.windows_error == Some(5)));
    std::thread::sleep(Duration::from_millis(100));
    let second = sampler.sample(1024);
    let own = second
        .processes
        .iter()
        .find(|p| p.pid == std::process::id())
        .unwrap();
    assert!(own.rates.cpu_percent_of_machine.unwrap() >= 0.0);
    assert!(own.rates.io_read_bytes_per_second.unwrap() >= 0.0);
    assert!(second.sample_interval_seconds.unwrap() >= 0.1);
    match second.disk_io {
        Observation::Observed { value } => {
            assert!(!value.is_empty());
            for disk in value {
                assert!(disk.sample_interval_seconds > 0.0);
                if let Some(rate) = disk.bytes_per_second {
                    assert!(rate.is_finite() && rate >= 0.0);
                } else {
                    assert!(disk.unknown_reason.is_some());
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
    std::thread::sleep(Duration::from_millis(100));
    let next = sampler.sample(1024);
    let own = next
        .processes
        .iter()
        .find(|p| p.pid == std::process::id())
        .unwrap();
    assert!(own.rates.cpu_percent_of_machine.is_some());
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
