use super::*;
use loomward_telemetry::{DiskIo, GpuAdapter, Memory, Observation, ProcessTotals, Rates, Unknown};
use serde_json::{json, to_value};

fn raw() -> Snapshot {
    Snapshot {
        schema_version: 2,
        status: "observed".into(),
        platform: "synthetic".into(),
        captured_at_unix_ms: 1_709_251_199_123,
        processes_observed_at_unix_ms: Some(1_709_251_199_123),
        sampled_channels: loomward_telemetry::SampleChannels::ALL,
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

fn live_lease(state: &mut State) -> SubscriptionId {
    let id = SubscriptionId::new("sb_synthetic").unwrap();
    let now = Instant::now();
    state.leases.insert(
        id.clone(),
        Lease {
            generation: 1,
            channels: CHANNELS.to_vec().try_into().unwrap(),
            interval: serde_json::from_value(json!(10000)).unwrap(),
            expires: now + LEASE_DURATION,
            next_event: now + Duration::from_secs(10),
        },
    );
    state.next_generation = 1;
    id
}

#[test]
fn unchanged_channel_renewal_preserves_queued_samples_and_generation() {
    let telemetry = Telemetry::default();
    let (id, next_event) = {
        let mut state = lock(&telemetry.shared.state).unwrap();
        let id = live_lease(&mut state);
        state.running = true; // Exercise renewal without starting native observation.
        state.events.push_back(TelemetrySampleEvent {
            subscription_id: id.clone(),
            sample: mapping::sample(&raw(), None, 1, CHANNELS, false).unwrap(),
        });
        (id.clone(), state.leases[&id].next_event)
    };
    let mut channels = CHANNELS.to_vec();
    channels.reverse(); // Channel order does not change the projection.
    let request = TelemetrySubscribeRequest {
        subscription_id: Some(id.clone()),
        channels: channels.try_into().unwrap(),
        interval_ms: serde_json::from_value(json!(10000)).unwrap(),
    };
    telemetry.lease(&request).unwrap();
    {
        let state = lock(&telemetry.shared.state).unwrap();
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.leases[&id].generation, 1);
        assert_eq!(state.leases[&id].next_event, next_event);
    }
    telemetry
        .lease(&TelemetrySubscribeRequest {
            interval_ms: serde_json::from_value(json!(1000)).unwrap(),
            ..request.clone()
        })
        .unwrap();
    assert_eq!(lock(&telemetry.shared.state).unwrap().events.len(), 1);
    telemetry
        .lease(&TelemetrySubscribeRequest {
            channels: vec![TelemetryChannel::Gpu].try_into().unwrap(),
            ..request
        })
        .unwrap();
    let state = lock(&telemetry.shared.state).unwrap();
    assert!(state.events.is_empty());
    assert_ne!(state.leases[&id].generation, 1);
}

#[test]
fn handle_denial_is_denied_and_field_denial_is_limited() {
    let at = mapping::processes_observed_at(&raw()).unwrap();
    for field in [
        "process_handle_fields",
        "io_counters",
        "handle_count",
        "session_id",
        "private_working_set_bytes",
        "memory",
        "start_and_cpu_times",
        "io_bytes",
    ] {
        let mut process = raw().processes.remove(0);
        process.protected_or_unknown = true;
        process.unknowns.push(Unknown {
            field: field.into(),
            reason: "access_denied".into(),
            windows_error: Some(5),
        });
        if field == "process_handle_fields" {
            process = Process {
                unknowns: process.unknowns,
                protected_or_unknown: true,
                ..Process::default()
            };
        } else {
            process.rates.cpu_fraction = Some(0.25);
        }
        let row = mapping::process_row(&process, 1, &at);
        assert_eq!(
            row.access,
            if field == "process_handle_fields" {
                ProcessRowAccess::Denied
            } else {
                ProcessRowAccess::Limited
            }
        );
        if field != "process_handle_fields" {
            assert_eq!(row.working_set_bytes, Some(Bytes::from(200)));
            assert_eq!(row.cpu_fraction.unwrap().get(), 0.25);
        }
    }
}

#[test]
fn expired_leases_make_all_retained_reads_unavailable_before_worker_wakes() {
    let mut outcomes = vec![];
    for read in 0..3 {
        let e = Engine::open(crate::EngineConfig::new(
            "unused-state".into(),
            DatasetClass::Synthetic,
        ))
        .unwrap();
        let reference = mapping::process_ref(&raw().processes[0], 1);
        {
            let mut state = lock(&e.telemetry.shared.state).unwrap();
            let id = live_lease(&mut state);
            state.leases.get_mut(&id).unwrap().expires = Instant::now();
            state.latest = Some(Latest {
                raw: raw(),
                own: None,
                sequence: 1,
            });
        }
        let result = match read {
            0 => e
                .telemetry_snapshot(
                    &serde_json::from_value(json!({"channels":["system"]})).unwrap(),
                )
                .map(|_| ()),
            1 => e
                .processes_list(
                    &serde_json::from_value(json!({"sort":"name_asc","limit":10})).unwrap(),
                )
                .map(|_| ()),
            _ => e
                .processes_explain(&ProcessRefRequest {
                    process_ref: reference,
                })
                .map(|_| ()),
        };
        outcomes.push((
            matches!(result, Err(EngineError::PartialCoverage { .. })),
            lock(&e.telemetry.shared.state).unwrap().latest.is_none(),
        ));
    }
    assert_eq!(outcomes, vec![(true, true); 3]);
}

#[test]
fn unchanged_renewal_during_mapping_preserves_the_pending_sample() {
    let telemetry = Telemetry::default();
    let (mapped_tx, mapped_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    {
        let mut state = lock(&telemetry.shared.state).unwrap();
        state.sample_override = Some(raw());
        state.mapping_pause = Some((mapped_tx, resume_rx));
    }
    let request: TelemetrySubscribeRequest = serde_json::from_value(
        json!({"subscription_id":null,"channels":["system"],"interval_ms":10000}),
    )
    .unwrap();
    let lease = telemetry.lease(&request).unwrap();
    mapped_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    telemetry
        .lease(&TelemetrySubscribeRequest {
            subscription_id: Some(lease.subscription_id.clone()),
            ..request
        })
        .unwrap();
    resume_tx.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let state = lock(&telemetry.shared.state).unwrap();
        if state.sequence > 0 {
            assert_eq!(state.events.len(), 1);
            assert_eq!(state.events[0].subscription_id, lease.subscription_id);
            break;
        }
        drop(state);
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    telemetry
        .release(&SubscriptionRefRequest {
            subscription_id: lease.subscription_id,
        })
        .unwrap();
}

#[test]
fn mapping_preserves_exact_bytes_units_and_distinct_quantities() {
    let mut raw = raw();
    raw.sample_interval_seconds = Some(2.5);
    let own = Process {
        pid: std::process::id(),
        private_commit_bytes: Some(777),
        working_set_bytes: Some(333),
        thread_count: Some(3),
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
    assert_eq!(mapping::process_ref(&a, 1), mapping::process_ref(&a, 2));
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
    raw.processes.reverse();
    for sort in [
        ProcessListRequestSort::PrivateDesc,
        ProcessListRequestSort::WorkingSetDesc,
        ProcessListRequestSort::CpuDesc,
        ProcessListRequestSort::IoDesc,
    ] {
        let rows = mapping::sorted_rows(&raw, 1, sort).unwrap();
        assert_eq!(rows[0].pid.get(), 7);
    }
    assert_eq!(
        mapping::sorted_rows(&raw, 1, ProcessListRequestSort::NameAsc).unwrap()[0]
            .pid
            .get(),
        8
    );
    let denied_index = raw.processes.iter().position(|p| p.pid == 8).unwrap();
    raw.processes[denied_index].unknowns.push(Unknown {
        field: "process_handle_fields".into(),
        reason: "access_denied".into(),
        windows_error: Some(5),
    });
    raw.processes[denied_index].protected_or_unknown = true;
    assert_eq!(
        mapping::process_row(
            &raw.processes[denied_index],
            1,
            &mapping::processes_observed_at(&raw).unwrap()
        )
        .access,
        ProcessRowAccess::Denied
    );
    let e = Engine::open(crate::EngineConfig {
        state_dir: "unused".into(),
        dataset_class: DatasetClass::Synthetic,
    })
    .unwrap();
    let reference = mapping::process_ref(&raw.processes[denied_index], 1);
    {
        let mut state = lock(&e.telemetry.shared.state).unwrap();
        live_lease(&mut state);
        state.latest = Some(Latest {
            raw,
            own: None,
            sequence: 1,
        });
    }
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
    // Longer than the 10 s interval: when the first collection predates the second lease, the first
    // shared sample is the next interval tick.
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        let state = lock(&telemetry.shared.state).unwrap();
        assert!(state.events.iter().all(|e| e.sample.system.is_none()));
        // One sampler serves both leases: some sample reaches both subscriptions. (The first sample
        // may predate the second lease and reach only the first; slow CI runners showed that.)
        let shared = state.events.iter().any(|a| {
            a.subscription_id == second.subscription_id
                && state.events.iter().any(|b| {
                    b.subscription_id == first.subscription_id
                        && b.sample.sample_seq == a.sample.sample_seq
                })
        });
        if shared {
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

#[test]
fn unknown_creation_ref_survives_list_to_explain_tick() {
    let e = Engine::open(crate::EngineConfig {
        state_dir: "unused".into(),
        dataset_class: DatasetClass::Synthetic,
    })
    .unwrap();
    let mut snapshot = raw();
    snapshot.processes[0].start_time_windows_100ns = None;
    let mut state = lock(&e.telemetry.shared.state).unwrap();
    live_lease(&mut state);
    state.latest = Some(Latest {
        raw: snapshot,
        own: None,
        sequence: 1,
    });
    drop(state);
    let list = e
        .processes_list(&serde_json::from_value(json!({"sort":"name_asc","limit":10})).unwrap())
        .unwrap();
    lock(&e.telemetry.shared.state)
        .unwrap()
        .latest
        .as_mut()
        .unwrap()
        .sequence = 2;
    let reference = list.rows[0].process_ref.clone();
    assert!(e
        .processes_explain(&ProcessRefRequest {
            process_ref: reference.clone()
        })
        .is_ok());
    let mut changed = raw().processes.remove(0);
    changed.start_time_windows_100ns = None;
    assert_eq!(reference, mapping::process_ref(&changed, 2));
    changed.parent_pid += 1;
    assert_ne!(reference, mapping::process_ref(&changed, 2));
    changed.parent_pid -= 1;
    changed.image_name = Some("Other.exe".into());
    assert_ne!(reference, mapping::process_ref(&changed, 2));
}

#[cfg(windows)]
#[test]
fn health_reads_own_usage_without_a_lease_or_retained_values() {
    let e = Engine::open(crate::EngineConfig {
        state_dir: "unused".into(),
        dataset_class: DatasetClass::Personal,
    })
    .unwrap();
    assert!(
        e.telemetry_health()
            .unwrap()
            .private_commit_bytes
            .unwrap()
            .get()
            > 0
    );
    lock(&e.telemetry.shared.state).unwrap().latest = Some(Latest {
        raw: raw(),
        own: Some(Process {
            private_commit_bytes: Some(1),
            ..Process::default()
        }),
        sequence: 1,
    });
    assert!(
        e.telemetry_health()
            .unwrap()
            .private_commit_bytes
            .unwrap()
            .get()
            > 1
    );
}

#[test]
fn sampler_exit_discards_latest_before_restart() {
    let telemetry = Telemetry::default();
    lock(&telemetry.shared.state).unwrap().latest = Some(Latest {
        raw: raw(),
        own: None,
        sequence: 1,
    });
    run(Arc::clone(&telemetry.shared));
    assert!(lock(&telemetry.shared.state).unwrap().latest.is_none());
}

#[test]
fn sampler_and_mapping_panics_reset_flags_and_allow_next_lease() {
    for mapping in [true, false] {
        let telemetry = Telemetry::default();
        {
            let mut state = lock(&telemetry.shared.state).unwrap();
            state.panic_sample = !mapping;
            state.panic_mapping = mapping;
        }
        let request = serde_json::from_value(
            json!({"subscription_id":null,"channels":["engine"],"interval_ms":10000}),
        )
        .unwrap();
        telemetry.lease(&request).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while !lock(&telemetry.worker)
            .unwrap()
            .as_ref()
            .unwrap()
            .is_finished()
        {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        let state = lock(&telemetry.shared.state).unwrap();
        assert!(!state.running && !state.busy);
        assert!(state.latest.is_none());
        drop(state);
        let restarted = telemetry.lease(&request).unwrap();
        assert!(lock(&telemetry.shared.state).unwrap().running);
        // Drop ends all surviving leases and joins the replacement.
        assert!(lock(&telemetry.shared.state)
            .unwrap()
            .leases
            .contains_key(&restarted.subscription_id));
    }
}

#[test]
fn first_summary_ranks_working_set_when_cpu_is_unknown() {
    let mut snapshot = raw();
    snapshot.processes.push(Process {
        pid: 99,
        working_set_bytes: Some(900),
        ..Process::default()
    });
    let sample = mapping::sample(&snapshot, None, 1, CHANNELS, false).unwrap();
    assert_eq!(sample.processes.unwrap().top[0].pid.get(), 99);
}

#[test]
fn sorting_reorders_known_values_and_pid_ties() {
    let mut snapshot = raw();
    snapshot.processes = vec![
        Process {
            pid: 99,
            image_name: Some("zeta".into()),
            private_commit_bytes: Some(1),
            working_set_bytes: Some(1),
            rates: Rates {
                cpu_fraction: Some(0.1),
                io_read_bytes_per_s: Some(1.0),
                io_write_bytes_per_s: Some(1.0),
                ..Rates::default()
            },
            ..Process::default()
        },
        Process {
            pid: 8,
            image_name: Some("alpha".into()),
            private_commit_bytes: Some(9),
            working_set_bytes: Some(9),
            rates: Rates {
                cpu_fraction: Some(0.9),
                io_read_bytes_per_s: Some(9.0),
                io_write_bytes_per_s: Some(9.0),
                ..Rates::default()
            },
            ..Process::default()
        },
    ];
    for sort in [
        ProcessListRequestSort::PrivateDesc,
        ProcessListRequestSort::WorkingSetDesc,
        ProcessListRequestSort::CpuDesc,
        ProcessListRequestSort::IoDesc,
        ProcessListRequestSort::NameAsc,
        ProcessListRequestSort::GpuDesc,
    ] {
        assert_eq!(
            mapping::sorted_rows(&snapshot, 1, sort).unwrap()[0]
                .pid
                .get(),
            8
        );
    }
    snapshot.processes[0] = Process {
        pid: 99,
        ..snapshot.processes[1].clone()
    };
    assert_eq!(
        mapping::sorted_rows(&snapshot, 1, ProcessListRequestSort::CpuDesc).unwrap()[0]
            .pid
            .get(),
        8
    );
}

#[test]
fn event_mapping_errors_are_counted_and_state_remains_readable() {
    let telemetry = Telemetry::default();
    let mut invalid = raw();
    invalid.gpu = Observation::Observed {
        value: vec![GpuAdapter {
            adapter_id: "x".repeat(16384),
            ..GpuAdapter::default()
        }],
    };
    lock(&telemetry.shared.state).unwrap().sample_override = Some(invalid);
    let request = serde_json::from_value(
        json!({"subscription_id":null,"channels":["gpu"],"interval_ms":10000}),
    )
    .unwrap();
    let lease = telemetry.lease(&request).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let state = lock(&telemetry.shared.state).unwrap();
        if state.sequence > 0 {
            assert_eq!(state.costs.mapping_errors, 1);
            assert!(state.events.is_empty());
            break;
        }
        drop(state);
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    telemetry
        .release(&SubscriptionRefRequest {
            subscription_id: lease.subscription_id,
        })
        .unwrap();
}

#[test]
fn expired_session_cannot_supply_the_next_lease() {
    let telemetry = Telemetry::default();
    let request = serde_json::from_value(
        json!({"subscription_id":null,"channels":["engine"],"interval_ms":10000}),
    )
    .unwrap();
    let first = telemetry.lease(&request).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while lock(&telemetry.shared.state).unwrap().latest.is_none() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    {
        let mut state = lock(&telemetry.shared.state).unwrap();
        state
            .leases
            .get_mut(&first.subscription_id)
            .unwrap()
            .expires = Instant::now();
        state.latest = Some(Latest {
            raw: raw(),
            own: None,
            sequence: 999,
        });
    }
    let next = telemetry.lease(&request).unwrap();
    let state = lock(&telemetry.shared.state).unwrap();
    assert!(state
        .latest
        .as_ref()
        .is_none_or(|latest| latest.sequence != 999));
    drop(state);
    telemetry
        .release(&SubscriptionRefRequest {
            subscription_id: next.subscription_id,
        })
        .unwrap();
}

#[test]
fn requested_but_unsampled_channel_is_partial_coverage() {
    let mut snapshot = raw();
    for channel in CHANNELS {
        snapshot.sampled_channels = loomward_telemetry::SampleChannels::ALL;
        match channel {
            TelemetryChannel::System => snapshot.sampled_channels.system = false,
            TelemetryChannel::Processes => snapshot.sampled_channels.processes = false,
            TelemetryChannel::Gpu => snapshot.sampled_channels.gpu = false,
            TelemetryChannel::Disks => snapshot.sampled_channels.disks = false,
            TelemetryChannel::Engine => snapshot.sampled_channels.engine = false,
        }
        assert!(matches!(
            mapping::sample(&snapshot, None, 1, &[*channel], false),
            Err(EngineError::PartialCoverage { .. })
        ));
    }
    snapshot.sampled_channels = loomward_telemetry::SampleChannels::ALL;
    snapshot.sampled_channels.system = true;
    snapshot.memory = Observation::Unknown {
        reason: "hardware query failed".into(),
    };
    assert!(mapping::sample(&snapshot, None, 1, &[TelemetryChannel::System], false).is_ok());
    snapshot.processes_observed_at_unix_ms = None;
    assert!(matches!(
        mapping::sample(&snapshot, None, 1, &[TelemetryChannel::Processes], false),
        Err(EngineError::PartialCoverage { .. })
    ));
}

#[test]
fn retained_process_rows_keep_their_older_observation_time() {
    let mut snapshot = raw();
    snapshot.captured_at_unix_ms += 4000;
    let expected_at = mapping::processes_observed_at(&snapshot).unwrap();
    let e = Engine::open(crate::EngineConfig::new(
        "unused-state".into(),
        DatasetClass::Personal,
    ))
    .unwrap();
    {
        let mut state = lock(&e.telemetry.shared.state).unwrap();
        live_lease(&mut state);
        state.latest = Some(Latest {
            raw: snapshot.clone(),
            own: None,
            sequence: 1,
        });
    }
    let list = e
        .processes_list(&serde_json::from_value(json!({"sort":"name_asc","limit":10})).unwrap())
        .unwrap();
    assert_eq!(list.observed_at, expected_at);
    let explanation = e
        .processes_explain(&ProcessRefRequest {
            process_ref: list.rows[0].process_ref.clone(),
        })
        .unwrap();
    assert_eq!(explanation.observed_at, expected_at);
    let sample = mapping::sample(&snapshot, None, 1, CHANNELS, false).unwrap();
    let row = &sample.processes.unwrap().top[0];
    assert_eq!(
        row.observed_at,
        mapping::processes_observed_at(&snapshot).unwrap()
    );
    assert_ne!(row.observed_at, sample.observed_at);
}

#[test]
fn renewal_during_mapping_discards_the_old_shape() {
    let telemetry = Telemetry::default();
    let (mapped_tx, mapped_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    {
        let mut state = lock(&telemetry.shared.state).unwrap();
        state.sample_override = Some(raw());
        state.mapping_pause = Some((mapped_tx, resume_rx));
    }
    let request: TelemetrySubscribeRequest = serde_json::from_value(
        json!({"subscription_id":null,"channels":["system"],"interval_ms":10000}),
    )
    .unwrap();
    let lease = telemetry.lease(&request).unwrap();
    mapped_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    telemetry
        .lease(&TelemetrySubscribeRequest {
            subscription_id: Some(lease.subscription_id.clone()),
            channels: vec![TelemetryChannel::Gpu].try_into().unwrap(),
            ..request
        })
        .unwrap();
    resume_tx.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let state = lock(&telemetry.shared.state).unwrap();
        if state.sequence > 0 {
            assert!(state.events.is_empty());
            break;
        }
        drop(state);
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    telemetry
        .release(&SubscriptionRefRequest {
            subscription_id: lease.subscription_id,
        })
        .unwrap();
}
