use super::*;
use loomward_telemetry::{DiskIo, GpuAdapter, Memory, Observation, ProcessTotals, Rates, Unknown};
use serde_json::{json, to_value};

fn raw() -> Snapshot {
    Snapshot {
        schema_version: 2,
        status: "observed".into(),
        platform: "synthetic".into(),
        captured_at_unix_ms: 1_709_251_199_123,
        snapshot_cost_ms: 1.0,
        sample_interval_seconds: None,
        logical_processor_count: Some(8),
        memory: Observation::Observed {
            value: Memory {
                total_bytes: Some(u64::MAX),
                available_bytes: Some(512),
                commit_bytes: Some(1024),
                commit_limit_bytes: Some(2048),
                load_fraction: Some(0.25),
                ..Memory::default()
            },
        },
        processes: vec![Process {
            pid: 7,
            image_name: Some("Synthetic.exe".into()),
            start_time_windows_100ns: Some("116444736000000001".into()),
            private_commit_bytes: Some(u64::MAX),
            working_set_bytes: Some(200),
            ..Process::default()
        }],
        process_totals: ProcessTotals {
            enumerated: 1,
            ..ProcessTotals::default()
        },
        display_truncated: false,
        system_cpu_busy_fraction: Some(0.5),
        gpu: Observation::Observed {
            value: vec![GpuAdapter {
                adapter_id: "luid_synthetic".into(),
                dedicated_used_bytes: Some(123),
                shared_used_bytes: Some(456),
                engine_busy_fraction: Some(0.7),
                ..GpuAdapter::default()
            }],
        },
        process_gpu: Observation::Unsupported {
            reason: "not collected".into(),
        },
        disk_io: Observation::Observed {
            value: vec![DiskIo {
                disk_label: "synthetic".into(),
                read_bytes_per_s: Some(4096.5),
                write_bytes_per_s: Some(0.0),
                busy_fraction: Some(0.9),
                queue_length: Some(3.5),
                ..DiskIo::default()
            }],
        },
        pdh_malformed_instances: 0,
        pdh_unknowns: vec![],
    }
}

const CHANNELS: &[TelemetryChannel] = &[
    TelemetryChannel::System,
    TelemetryChannel::Processes,
    TelemetryChannel::Gpu,
    TelemetryChannel::Disks,
    TelemetryChannel::Engine,
];

#[test]
fn mapping_preserves_exact_bytes_units_and_distinct_quantities() {
    let mut raw = raw();
    raw.sample_interval_seconds = Some(2.5);
    let own = Process {
        pid: std::process::id(),
        private_commit_bytes: Some(777),
        working_set_bytes: Some(333),
        thread_count: 3,
        cpu_user_100ns: Some(10_000_000),
        cpu_kernel_100ns: Some(5_000_000),
        rates: Rates {
            cpu_fraction: Some(0.125),
            ..Rates::default()
        },
        ..Process::default()
    };
    let sample = mapping::sample(&raw, Some(&own), 2, CHANNELS, false).unwrap();
    let wire = to_value(&sample).unwrap();
    assert_eq!(
        wire["system"]["memory"]["total_bytes"],
        u64::MAX.to_string()
    );
    assert_eq!(wire["system"]["cpu"]["busy_fraction"], 0.5);
    assert_eq!(wire["elapsed_ms"], 2500);
    assert_eq!(
        wire["processes"]["top"][0]["started_at"],
        "1970-01-01T00:00:00.0000001Z"
    );
    assert_eq!(
        wire["processes"]["top"][0]["private_commit_bytes"],
        u64::MAX.to_string()
    );
    assert_eq!(wire["disks"]["disks"][0]["read_bytes_per_s"], 4096.5);
    assert_eq!(wire["disks"]["disks"][0]["busy_fraction"], 0.9);
    assert_eq!(wire["disks"]["disks"][0]["queue_length"], 3.5);
    assert_eq!(wire["gpu"]["adapters"][0]["dedicated_used_bytes"], "123");
    assert!(wire["gpu"]["adapters"][0]["dedicated_total_bytes"].is_null());
    assert_eq!(wire["engine"]["private_commit_bytes"], "777");
    assert_eq!(wire["engine"]["working_set_bytes"], "333");
    assert_eq!(wire["engine"]["cpu_fraction"], 0.125);
    assert_eq!(mapping::health(Some(&own)).cpu_seconds.unwrap().get(), 1.5);
    assert_eq!(sample, serde_json::from_value(wire).unwrap());
}

#[test]
fn missing_invalid_and_unsupported_stay_unknown_not_zero() {
    let mut raw = raw();
    raw.memory = Observation::Unknown {
        reason: "access_denied".into(),
    };
    raw.gpu = Observation::Unknown {
        reason: "counter_unavailable".into(),
    };
    raw.disk_io = Observation::Unsupported {
        reason: "not supported".into(),
    };
    raw.system_cpu_busy_fraction = Some(f64::NAN);
    let sample = mapping::sample(&raw, None, 1, CHANNELS, false).unwrap();
    assert!(sample.elapsed_ms.is_none());
    assert!(sample.system.as_ref().unwrap().memory.total_bytes.is_none());
    assert!(sample.system.as_ref().unwrap().cpu.busy_fraction.is_none());
    assert_eq!(sample.gpu.unwrap().state, GpuSampleState::Unavailable);
    assert_eq!(sample.disks.unwrap().state, DiskSampleState::Unavailable);
    assert!(sample.engine.unwrap().private_commit_bytes.is_none());
    let row = &sample.processes.unwrap().top[0];
    assert!(row.cpu_fraction.is_none());
    assert!(row.private_working_set_bytes.is_none());
    assert!(row.gpu_dedicated_bytes.is_none());
    raw.logical_processor_count = None;
    raw.status = "unsupported".into();
    let sample = mapping::sample(&raw, None, 1, CHANNELS, false).unwrap();
    assert!(sample.system.is_none());
    assert!(sample.processes.is_none());
    assert!(
        mapping::sample(&raw, None, 1, &[TelemetryChannel::Engine], false)
            .unwrap()
            .gpu
            .is_none()
    );
}

#[test]
fn calendar_and_filetime_are_exact_and_pid_reuse_does_not_reconnect() {
    assert_eq!(
        mapping::timestamp(-116_444_736_000_000_000)
            .unwrap()
            .as_str(),
        "1601-01-01T00:00:00.0000000Z"
    );
    assert_eq!(
        mapping::timestamp(1_709_251_200_000 * 10_000)
            .unwrap()
            .as_str(),
        "2024-03-01T00:00:00.0000000Z"
    );
    assert_eq!(
        mapping::timestamp(-1).unwrap().as_str(),
        "1969-12-31T23:59:59.9999999Z"
    );
    let mut a = raw().processes.remove(0);
    let original = mapping::process_ref(&a, 1);
    assert_eq!(original, mapping::process_ref(&a, 2));
    a.start_time_windows_100ns = Some("116444736000000002".into());
    assert_ne!(original, mapping::process_ref(&a, 2));
    a.start_time_windows_100ns = None;
    assert_ne!(mapping::process_ref(&a, 1), mapping::process_ref(&a, 2));
}

#[test]
fn sorting_puts_missing_values_last_and_explains_denial_without_actions() {
    let mut raw = raw();
    raw.processes[0].rates = Rates {
        cpu_fraction: Some(0.25),
        io_read_bytes_per_s: Some(100.0),
        io_write_bytes_per_s: Some(50.0),
        ..Rates::default()
    };
    raw.processes.push(Process {
        pid: 8,
        image_name: Some("aaaa".into()),
        ..Process::default()
    });
    for sort in [
        ProcessListRequestSort::PrivateDesc,
        ProcessListRequestSort::WorkingSetDesc,
        ProcessListRequestSort::CpuDesc,
        ProcessListRequestSort::IoDesc,
    ] {
        let rows = mapping::sorted_rows(&raw, 1, sort);
        assert_eq!(rows[0].pid.get(), 7);
    }
    assert_eq!(
        mapping::sorted_rows(&raw, 1, ProcessListRequestSort::NameAsc)[0]
            .pid
            .get(),
        8
    );
    raw.processes[1].unknowns.push(Unknown {
        field: "process_handle_fields".into(),
        reason: "access_denied".into(),
        windows_error: Some(5),
    });
    raw.processes[1].protected_or_unknown = true;
    assert_eq!(
        mapping::process_row(&raw.processes[1], 1).access,
        ProcessRowAccess::Denied
    );
    let e = Engine::open(crate::EngineConfig {
        state_dir: "unused".into(),
        dataset_class: DatasetClass::Synthetic,
    })
    .unwrap();
    let reference = mapping::process_ref(&raw.processes[1], 1);
    lock(&e.telemetry.shared.state).unwrap().latest = Some(Latest {
        raw,
        own: None,
        sequence: 1,
    });
    let explanation = e
        .processes_explain(&ProcessRefRequest {
            process_ref: reference,
        })
        .unwrap();
    assert!(explanation
        .facts
        .iter()
        .any(|f| f.text.as_str().contains("access_denied")));
    assert!(explanation.available_actions.is_empty());
    assert!(
        e.processes_list(&serde_json::from_value(json!({"sort":"cpu_desc","limit":1})).unwrap())
            .unwrap()
            .truncated
    );
}

#[test]
fn expiry_removes_renewal_right_and_wakes_the_last_sampler() {
    let telemetry = Telemetry::default();
    let request: TelemetrySubscribeRequest = serde_json::from_value(
        json!({"subscription_id":null,"channels":["engine"],"interval_ms":10000}),
    )
    .unwrap();
    let lease = telemetry.lease(&request).unwrap();
    {
        let mut state = lock(&telemetry.shared.state).unwrap();
        state
            .leases
            .get_mut(&lease.subscription_id)
            .unwrap()
            .expires = Instant::now() + Duration::from_millis(50);
    }
    telemetry.shared.wake.notify_one();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let state = lock(&telemetry.shared.state).unwrap();
        if !state.running {
            assert!(state.leases.is_empty());
            assert!(state.events.is_empty());
            break;
        }
        drop(state);
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        telemetry.lease(&TelemetrySubscribeRequest {
            subscription_id: Some(lease.subscription_id.clone()),
            ..request.clone()
        }),
        Err(EngineError::NotFound { .. })
    ));
    assert!(matches!(
        telemetry.release(&SubscriptionRefRequest {
            subscription_id: lease.subscription_id
        }),
        Err(EngineError::NotFound { .. })
    ));
    let restarted = telemetry.lease(&request).unwrap();
    assert!(lock(&telemetry.shared.state).unwrap().running);
    telemetry
        .release(&SubscriptionRefRequest {
            subscription_id: restarted.subscription_id,
        })
        .unwrap();
    assert!(!lock(&telemetry.shared.state).unwrap().running);
}

#[test]
fn leases_reuse_one_thread_and_enforce_a_bounded_60_second_lifetime() {
    let telemetry = Telemetry::default();
    let request: TelemetrySubscribeRequest = serde_json::from_value(json!({
        "subscription_id":null,"channels":["engine"],"interval_ms":10000
    }))
    .unwrap();
    let before = Instant::now();
    let first = telemetry.lease(&request).unwrap();
    let worker_id = lock(&telemetry.worker)
        .unwrap()
        .as_ref()
        .unwrap()
        .thread()
        .id();
    let lifetime = lock(&telemetry.shared.state).unwrap().leases[&first.subscription_id]
        .expires
        .duration_since(before);
    assert!(lifetime >= Duration::from_secs(60));
    assert!(lifetime < Duration::from_secs(61));
    let second = telemetry.lease(&request).unwrap();
    assert_eq!(
        worker_id,
        lock(&telemetry.worker)
            .unwrap()
            .as_ref()
            .unwrap()
            .thread()
            .id()
    );
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let state = lock(&telemetry.shared.state).unwrap();
        if state.events.len() >= 2 {
            assert!(state.events.iter().all(|e| e.sample.system.is_none()));
            assert!(state
                .events
                .iter()
                .all(|e| e.sample.sample_seq == state.events[0].sample.sample_seq));
            break;
        }
        drop(state);
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    telemetry
        .release(&SubscriptionRefRequest {
            subscription_id: first.subscription_id,
        })
        .unwrap();
    assert!(lock(&telemetry.shared.state).unwrap().running);
    assert!(lock(&telemetry.shared.state)
        .unwrap()
        .events
        .iter()
        .all(|e| e.subscription_id == second.subscription_id));
    for _ in 1..MAX_LEASES {
        telemetry.lease(&request).unwrap();
    }
    assert!(matches!(
        telemetry.lease(&request),
        Err(EngineError::ResourceBudget { .. })
    ));
    // Drop joins the one worker regardless of the number of unexpired leases.
}
