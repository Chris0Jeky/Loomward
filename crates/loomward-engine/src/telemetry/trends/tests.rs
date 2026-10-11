use super::*;
use crate::telemetry::tests::{raw, CHANNELS};
use loomward_telemetry::{Memory, Observation, Unknown};

fn series(values: &[Option<u64>]) -> History {
    let mut history = History::default();
    for (i, value) in values.iter().enumerate() {
        let mut raw = raw();
        raw.captured_at_unix_ms += i as u64 * 15_000;
        raw.processes_observed_at_unix_ms = Some(raw.captured_at_unix_ms);
        raw.processes[0].private_commit_bytes = *value;
        history.push(&raw, i as u64 * 15_000, 15_000);
    }
    history
}

fn growth(h: &History) -> Vec<TrendCandidate> {
    h.report(CHANNELS)
        .candidates
        .iter()
        .filter(|c| c.kind == TrendCandidateKind::Growth)
        .cloned()
        .collect()
}

#[test]
fn growth_requires_sustained_window_and_cites_exact_bytes() {
    assert!(growth(&series(&[Some(0), Some(1 << 20), Some(2 << 20)])).is_empty());
    let h = series(&[Some(0), Some(1 << 20), Some(2 << 20), Some(3 << 20)]);
    let c = growth(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].sample_count.get(), 4);
    assert_eq!(c[0].window_ms.get(), 45_000);
    assert_eq!(c[0].unit, TrendUnit::Bytes);
    assert_eq!(c[0].start_value, TrendValue::Bytes(0u64.into()));
    assert_eq!(c[0].end_value, TrendValue::Bytes((3u64 << 20).into()));
    assert!((c[0].slope_per_s.as_str().parse::<f64>().unwrap() - 69905.066667).abs() < 0.01);
    assert!(!c[0].facts.is_empty());
    assert!(c[0].unknowns.iter().any(|u| u.as_str().contains("Shared")));
}

#[test]
fn steady_sawtooth_and_small_growth_are_not_candidates() {
    for v in [
        [10, 10, 10, 10],
        [0, 4 << 20, 1 << 20, 5 << 20],
        [0, 1, 2, 3],
        [0, 0, 0, 4 << 20],
    ] {
        assert!(growth(&series(&v.map(Some))).is_empty());
    }
    assert!(growth(&series(&[0, 4 << 20, 1 << 20, 5 << 20, 6 << 20].map(Some))).is_empty());
    assert_eq!(
        series(&[Some(10); 4])
            .report(CHANNELS)
            .series
            .iter()
            .find(|s| s.metric == TrendMetric::PrivateCommitBytes)
            .unwrap()
            .points
            .len(),
        4
    );
}

#[test]
fn gaps_missing_and_cached_process_observations_do_not_become_growth() {
    let h = series(&[Some(0), Some(1 << 20), None, Some(3 << 20)]);
    assert!(growth(&h).is_empty());
    assert!(h
        .report(CHANNELS)
        .series
        .iter()
        .any(|s| s.points.iter().any(|p| p.value.is_none())));
    let mut h = series(&[Some(0), Some(1 << 20), Some(2 << 20)]);
    let mut r = raw();
    r.processes[0].private_commit_bytes = Some(3 << 20);
    r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms + 30_000);
    h.push(&r, 45_000, 15_000); // cached third observation, not a fourth sample
    assert!(growth(&h).is_empty());
    r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms + 120_000);
    h.push(&r, 120_000, 15_000);
    assert!(growth(&h).is_empty());
    assert!(h
        .report(CHANNELS)
        .series
        .iter()
        .any(|s| s.unknowns.iter().any(|u| u.as_str().contains("gap"))));
}

#[test]
fn pid_reuse_missing_identity_and_denied_handles_break_continuity() {
    for case in ["reuse", "identity", "denied", "disappeared", "enumeration"] {
        let mut h = series(&[Some(0), Some(1 << 20), Some(2 << 20)]);
        let mut r = raw();
        r.captured_at_unix_ms += 45_000;
        r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
        r.processes[0].private_commit_bytes = Some(3 << 20);
        match case {
            "reuse" => r.processes[0].start_time_windows_100ns = Some("116444736000000002".into()),
            "identity" => r.processes[0].start_time_windows_100ns = None,
            "denied" => r.processes[0].unknowns.push(Unknown {
                field: "process_handle_fields".into(),
                reason: "access_denied".into(),
                windows_error: Some(5),
            }),
            "enumeration" => r.process_totals.enumeration_unknowns.push(Unknown {
                field: "enumeration".into(),
                reason: "provider_failed".into(),
                windows_error: None,
            }),
            _ => r.processes.clear(),
        }
        h.push(&r, 45_000, 15_000);
        assert!(growth(&h).is_empty(), "{case}");
        let report = h.report(CHANNELS);
        assert_eq!(
            report
                .series
                .iter()
                .filter(|s| s.metric.channel() == TelemetryChannel::Processes)
                .count(),
            if case == "disappeared" { 0 } else { 7 },
            "{case}: ended instance retained"
        );
        assert!(
            !report.unknowns.is_empty()
                || report
                    .series
                    .iter()
                    .any(|s| s.points.iter().any(|p| p.value.is_none()))
        );
    }
}

#[test]
fn contention_uses_fraction_queue_and_memory_movement_not_busy_as_queue() {
    let mut h = History::default();
    for i in 0..4 {
        let mut r = raw();
        r.captured_at_unix_ms += i * 1000;
        r.system_cpu_busy_fraction = Some(0.95);
        r.memory = Observation::Observed {
            value: Memory {
                total_bytes: Some(1000),
                available_bytes: Some(50),
                standby_bytes: Some(300 - i * 10),
                modified_bytes: Some(10 + i * 10),
                ..Memory::default()
            },
        };
        h.push(&r, i * 1000, 1000);
    }
    let report = h.report(CHANNELS);
    for kind in [
        TrendCandidateKind::CpuSaturation,
        TrendCandidateKind::MemoryPressure,
        TrendCandidateKind::DiskQueue,
    ] {
        assert!(report.candidates.iter().any(|c| c.kind == kind), "{kind:?}");
    }
    assert_eq!(
        report
            .candidates
            .iter()
            .find(|c| c.kind == TrendCandidateKind::DiskQueue)
            .unwrap()
            .unit,
        TrendUnit::Queue
    );
    assert!(report
        .candidates
        .iter()
        .find(|c| c.kind == TrendCandidateKind::MemoryPressure)
        .unwrap()
        .facts
        .iter()
        .any(|f| f.as_str().contains("standby")));
    let modified = h
        .series
        .get_mut(&("system".into(), TrendMetric::ModifiedBytes))
        .unwrap();
    let modified = Arc::make_mut(modified);
    let mut points = std::mem::take(&mut modified.points).into_vec();
    for p in &mut points {
        p.value = None;
    }
    modified.points = points.try_into().unwrap();
    assert!(h
        .report(CHANNELS)
        .candidates
        .iter()
        .find(|c| c.kind == TrendCandidateKind::MemoryPressure)
        .unwrap()
        .unknowns
        .iter()
        .any(|u| u.as_str().contains("ModifiedBytes")));
}

#[test]
fn unit_mismatches_are_unknown_and_wire_rejected() {
    let mut r = raw();
    r.system_cpu_busy_fraction = Some(90.0); // percent is not a fraction
    if let Observation::Observed { value } = &mut r.disk_io {
        value[0].busy_fraction = Some(3.5);
    }
    let mut h = History::default();
    h.push(&r, 0, 1000);
    let report = h.report(CHANNELS);
    for metric in [TrendMetric::CpuBusyFraction, TrendMetric::DiskBusyFraction] {
        assert!(report
            .series
            .iter()
            .find(|s| s.metric == metric)
            .unwrap()
            .points[0]
            .value
            .is_none());
    }
    let s = report
        .series
        .iter()
        .find(|s| s.metric == TrendMetric::DiskQueueLength)
        .unwrap();
    assert!(s.points[0].value.is_some()); // 3.5 is valid queue length
    let mut value = serde_json::to_value(s).unwrap();
    value["unit"] = serde_json::json!("bytes_per_s");
    assert!(serde_json::from_value::<TrendSeries>(value).is_err());
}

#[test]
fn history_is_bounded_projected_and_channel_retention_clears_it() {
    let mut h = series(&[Some(10); 30]);
    let report = h.report(&[TelemetryChannel::Processes]);
    assert_eq!(report.series.len(), 7);
    assert!(report.series.iter().all(|s| s.points.len() <= 8));
    h.retain_channels(&[TelemetryChannel::System]);
    assert!(h.report(&[TelemetryChannel::Processes]).series.is_empty());
    h.retain_channels(&[]);
    h.push(&raw(), 600_000, 15_000);
    assert!(h.report(CHANNELS).series.is_empty());
    let mut r = raw();
    r.processes = (0..1025)
        .map(|pid| loomward_telemetry::Process {
            pid,
            ..r.processes[0].clone()
        })
        .collect();
    if let Observation::Observed { value } = &mut r.disk_io {
        *value = (0..17)
            .map(|i| loomward_telemetry::DiskIo {
                disk_label: format!("synthetic-{i}"),
                ..value[0].clone()
            })
            .collect();
    }
    let mut bounded = History::default();
    bounded.push(&r, 0, 1000);
    assert_eq!(bounded.report(CHANNELS).series.len(), 7213);
}

#[test]
fn missing_process_provider_breaks_old_candidates_and_large_bytes_keep_small_deltas() {
    let mut h = series(&[Some(0), Some(1 << 20), Some(2 << 20), Some(3 << 20)]);
    assert_eq!(growth(&h).len(), 1);
    let mut r = raw();
    r.captured_at_unix_ms += 60_000;
    r.processes_observed_at_unix_ms = None;
    h.push(&r, 60_000, 15_000);
    assert!(growth(&h).is_empty());
    assert!(!h.report(CHANNELS).unknowns.is_empty());
    let top = u64::MAX;
    let step = (1 << 20) + 1;
    let h = series(&[
        Some(top - 3 * step),
        Some(top - 2 * step),
        Some(top - step),
        Some(top),
    ]);
    assert!(
        (growth(&h)[0].slope_per_s.as_str().parse::<f64>().unwrap() - 69905.133333).abs() < 0.01
    );
}

#[test]
fn diagnostic_micro_measurement_bounds_payload_and_times_full_synthetic_history() {
    let mut r = raw();
    r.processes = (0..1024)
        .map(|pid| loomward_telemetry::Process {
            pid,
            start_time_windows_100ns: Some("116444736000000001".into()),
            private_commit_bytes: Some(u64::MAX),
            private_working_set_bytes: Some(u64::MAX),
            working_set_bytes: Some(u64::MAX),
            rates: loomward_telemetry::Rates {
                cpu_fraction: Some(0.5),
                io_read_bytes_per_s: Some(1024.0),
                io_write_bytes_per_s: Some(1024.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .collect();
    let mut h = History::default();
    let start = std::time::Instant::now();
    for i in 0..16 {
        r.captured_at_unix_ms += 15000;
        r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
        h.push(&r, i * 15000, 15000);
        let report = h.report(CHANNELS);
        assert_eq!(report.series.len(), 7183);
        std::hint::black_box(report);
    }
    let cost_ms = start.elapsed().as_secs_f64() * 1000.0 / 16.0;
    let report = h.report(CHANNELS);
    let size = serde_json::to_vec(&report).unwrap().len();
    assert!(
        size < 8 * 1024 * 1024,
        "trend report exceeds HTTP response bound: {size}"
    );
    assert!(cost_ms.is_finite() && cost_ms > 0.0);
    eprintln!("LW051 synthetic 1024-row push+report: {cost_ms:.3} ms/sample; {size} serialized bytes; shared-host diagnostic, not P12");
}

#[test]
fn threshold_duration_and_memory_evidence_guards_refuse_counterexamples() {
    let mut three = History::default();
    let mut r = raw();
    r.system_cpu_busy_fraction = Some(0.95);
    for t in [0, 1500, 3000] {
        three.push(&r, t, 2000);
    }
    assert!(three.report(CHANNELS).candidates.is_empty());
    for case in [
        "cpu_low",
        "queue_low",
        "available_high",
        "total_missing",
        "movement_missing",
        "short",
        "available_gap",
    ] {
        let mut h = History::default();
        for i in 0..4 {
            let mut r = raw();
            r.captured_at_unix_ms += i * 1000;
            r.system_cpu_busy_fraction = Some(if case == "cpu_low" { 0.89 } else { 0.95 });
            if let Observation::Observed { value } = &mut r.disk_io {
                value[0].queue_length = Some(if case == "queue_low" { 1.99 } else { 3.0 });
            }
            r.memory = Observation::Observed {
                value: Memory {
                    total_bytes: if case == "total_missing" {
                        None
                    } else {
                        Some(1000)
                    },
                    available_bytes: if case == "available_gap" && i == 2 {
                        None
                    } else {
                        Some(if case == "available_high" { 101 } else { 50 })
                    },
                    standby_bytes: Some(if case == "movement_missing" {
                        300
                    } else {
                        300 - i * 10
                    }),
                    modified_bytes: None,
                    ..Memory::default()
                },
            };
            h.push(&r, i * if case == "short" { 500 } else { 1000 }, 1000);
        }
        let report = h.report(CHANNELS);
        if case == "short" {
            assert!(report.candidates.is_empty());
        }
        let refused = match case {
            "cpu_low" => TrendCandidateKind::CpuSaturation,
            "queue_low" => TrendCandidateKind::DiskQueue,
            _ => TrendCandidateKind::MemoryPressure,
        };
        assert!(
            !report.candidates.iter().any(|c| c.kind == refused),
            "{case}"
        );
    }
    let mut h = History::default();
    for i in 0..4 {
        let mut r = raw();
        r.captured_at_unix_ms += i * 5000;
        r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
        r.processes[0].private_commit_bytes = Some(i << 20);
        h.push(&r, i * 5000, 5000);
    }
    assert!(growth(&h).is_empty()); // four samples are insufficient without the 30 s window
}

#[test]
fn expired_points_and_nonmonotonic_time_do_not_form_windows() {
    let mut h = series(&[Some(0), Some(1 << 20), Some(2 << 20)]);
    let mut r = raw();
    r.captured_at_unix_ms += 1_000_000;
    r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
    r.processes[0].private_commit_bytes = Some(3 << 20);
    h.push(&r, 1_000_000, 15000);
    assert!(growth(&h).is_empty());
    assert_eq!(
        h.report(CHANNELS)
            .series
            .iter()
            .find(|s| s.metric == TrendMetric::PrivateCommitBytes)
            .unwrap()
            .points
            .len(),
        1
    );
    for t in [1_015_000, 1_015_000, 1_045_000] {
        r.captured_at_unix_ms += 1000;
        r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
        r.processes[0].private_commit_bytes =
            r.processes[0].private_commit_bytes.map(|v| v + (1 << 20));
        h.push(&r, t, 15000);
    }
    assert!(growth(&h).is_empty());
}

#[test]
fn disk_churn_is_bounded_and_duplicate_labels_never_claim_continuity() {
    let mut h = History::default();
    for i in 0..40 {
        let mut r = raw();
        if let Observation::Observed { value } = &mut r.disk_io {
            value[0].disk_label = format!("synthetic-{i}");
        }
        h.push(&r, i * 1000, 1000);
    }
    assert_eq!(h.report(&[TelemetryChannel::Disks]).series.len(), 2);
    let mut r = raw();
    if let Observation::Observed { value } = &mut r.disk_io {
        value.push(value[0].clone());
    }
    let mut h = History::default();
    for i in 0..4 {
        h.push(&r, i * 1000, 1000);
    }
    let report = h.report(&[TelemetryChannel::Disks]);
    assert_eq!(report.series.len(), 4);
    assert!(report.candidates.is_empty());
    assert!(report
        .series
        .iter()
        .all(|s| s.points.iter().all(|p| p.value.is_none())));
    h.retain_channels(&[TelemetryChannel::System]);
    h.push(&r, 5000, 1000);
    assert!(h
        .report(&[TelemetryChannel::Processes, TelemetryChannel::Disks])
        .series
        .is_empty());
}

#[test]
fn unknown_or_malformed_creation_times_never_authorize_growth_comparison() {
    for identity in [
        None,
        Some("+116444736000000001"),
        Some("not-a-time"),
        Some("18446744073709551616"),
    ] {
        let mut h = History::default();
        for i in 0..4 {
            let mut r = raw();
            r.captured_at_unix_ms += i * 15000;
            r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
            r.processes[0].start_time_windows_100ns = identity.map(str::to_owned);
            r.processes[0].private_commit_bytes = Some(i << 20);
            h.push(&r, i * 15000, 15000);
        }
        assert!(growth(&h).is_empty(), "{identity:?}");
        assert!(h
            .report(CHANNELS)
            .series
            .iter()
            .find(|s| s.metric == TrendMetric::PrivateCommitBytes)
            .unwrap()
            .points
            .iter()
            .all(|p| p.value.is_none()));
    }
}

#[test]
fn cadence_changes_start_a_new_window_instead_of_rehabilitating_a_gap() {
    let mut h = History::default();
    let mut r = raw();
    r.system_cpu_busy_fraction = Some(0.99);
    for t in [0, 1000, 4000] {
        h.push(&r, t, 1000);
    }
    h.push(&r, 7000, 10_000);
    let report = h.report(CHANNELS);
    assert!(!report
        .candidates
        .iter()
        .any(|c| c.kind == TrendCandidateKind::CpuSaturation));
    let cpu = report
        .series
        .iter()
        .find(|s| s.metric == TrendMetric::CpuBusyFraction)
        .unwrap();
    assert_eq!(cpu.points.len(), 1);
    assert!(cpu.unknowns.iter().any(|u| u.as_str().contains("Cadence")));
    for t in [17_000, 27_000, 37_000] {
        h.push(&r, t, 10_000);
    }
    assert!(h
        .report(CHANNELS)
        .candidates
        .iter()
        .any(|c| c.kind == TrendCandidateKind::CpuSaturation));
}

#[test]
fn candidate_cap_and_aligned_memory_windows_remain_explicit() {
    let mut h = History::default();
    for i in 0..4 {
        let mut r = raw();
        r.captured_at_unix_ms += i * 15000;
        r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
        r.processes = (0..65)
            .map(|pid| loomward_telemetry::Process {
                pid,
                private_commit_bytes: Some(i << 20),
                ..r.processes[0].clone()
            })
            .collect();
        r.memory = Observation::Observed {
            value: Memory {
                total_bytes: Some(1000),
                available_bytes: Some(50),
                standby_bytes: Some(300 - i * 10),
                ..Default::default()
            },
        };
        h.push(&r, i * 15000, 15000);
    }
    let report = h.report(CHANNELS);
    assert_eq!(report.candidates.len(), 64);
    assert!(report.candidates_truncated);
    assert!(report
        .candidates
        .iter()
        .all(|c| (c.series_index.get() as usize) < report.series.len()));
    assert!(h
        .report(&[TelemetryChannel::System])
        .candidates
        .iter()
        .any(|c| c.kind == TrendCandidateKind::MemoryPressure));
    let total = h
        .series
        .get_mut(&("system".into(), TrendMetric::TotalBytes))
        .unwrap();
    let total = Arc::make_mut(total);
    let mut points = std::mem::take(&mut total.points).into_vec();
    points[0].offset_ms = Count::from(1u32);
    total.points = points.try_into().unwrap();
    assert!(!h
        .report(&[TelemetryChannel::System])
        .candidates
        .iter()
        .any(|c| c.kind == TrendCandidateKind::MemoryPressure));
}

#[test]
fn reports_share_unchanged_histories_but_keep_prior_observations_immutable() {
    let mut h = series(&[Some(0), Some(1 << 20), Some(2 << 20), Some(3 << 20)]);
    let before = h.report(&[TelemetryChannel::Processes]);
    let again = h.report(&[TelemetryChannel::Processes]);
    let series_at = |report: &TelemetryTrends| {
        report
            .series
            .iter()
            .position(|s| s.metric == TrendMetric::PrivateCommitBytes)
            .unwrap()
    };
    let i = series_at(&before);
    assert_eq!(
        before.series[i].points.as_ptr(),
        again.series[i].points.as_ptr()
    );
    let mut r = raw();
    r.captured_at_unix_ms += 60_000;
    r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
    r.processes[0].private_commit_bytes = Some(4 << 20);
    h.push(&r, 60_000, 15000);
    let after = h.report(&[TelemetryChannel::Processes]);
    assert_eq!(before.series[i].points.len(), 4);
    assert_eq!(after.series[series_at(&after)].points.len(), 5);
    assert_eq!(
        before.series[i].points.last().unwrap().value,
        Some(TrendValue::Bytes((3u64 << 20).into()))
    );
}

#[test]
fn event_histories_fit_the_sse_line_and_reindex_candidates_without_losing_snapshot_coverage() {
    let mut h = History::default();
    let mut r = raw();
    r.system_cpu_busy_fraction = Some(0.95);
    r.processes = (0..1024)
        .map(|pid| loomward_telemetry::Process {
            pid,
            private_working_set_bytes: Some(u64::MAX),
            working_set_bytes: Some(u64::MAX),
            rates: loomward_telemetry::Rates {
                cpu_fraction: Some(0.5),
                io_read_bytes_per_s: Some(1024.0),
                io_write_bytes_per_s: Some(1024.0),
                ..Default::default()
            },
            ..r.processes[0].clone()
        })
        .collect();
    for i in 0..8 {
        r.captured_at_unix_ms += 15000;
        r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
        r.processes[0].private_commit_bytes = Some(i << 20);
        r.processes[999].private_commit_bytes = Some(i << 20);
        r.memory = Observation::Observed {
            value: Memory {
                total_bytes: Some(1000),
                available_bytes: Some(50),
                standby_bytes: Some(300 - i * 10),
                ..Default::default()
            },
        };
        h.push(&r, i * 15000, 15000);
    }
    let snapshot = h.report(CHANNELS);
    assert_eq!(snapshot.series.len(), 7183);
    assert_eq!(growth(&h).len(), 2);
    let mut sample = mapping::sample(&r, None, 8, CHANNELS, false).unwrap();
    sample.trends = event_projection(snapshot.clone(), sample.processes.as_ref());
    let event_bytes = serde_json::to_vec(&sample).unwrap().len();
    eprintln!(
        "LW051 synthetic event: {event_bytes} bytes; snapshot retains {} series",
        snapshot.series.len()
    );
    assert!(
        event_bytes < (1 << 20) - 4096,
        "event leaves no room for SSE envelope below the line limit"
    );
    assert_eq!(
        sample
            .trends
            .series
            .iter()
            .filter(|s| s.metric.channel() == TelemetryChannel::Processes)
            .count(),
        140
    );
    assert!(sample
        .trends
        .unknowns
        .iter()
        .any(|u| u.as_str().contains("snapshot")));
    assert_eq!(
        sample
            .trends
            .candidates
            .iter()
            .filter(|c| c.kind == TrendCandidateKind::Growth)
            .count(),
        1
    );
    for c in &sample.trends.candidates {
        let source = &sample.trends.series[c.series_index.get() as usize];
        let expected = match c.kind {
            TrendCandidateKind::Growth => TrendMetric::PrivateCommitBytes,
            TrendCandidateKind::CpuSaturation => TrendMetric::CpuBusyFraction,
            TrendCandidateKind::MemoryPressure => TrendMetric::AvailableBytes,
            TrendCandidateKind::DiskQueue => TrendMetric::DiskQueueLength,
        };
        assert_eq!(source.metric, expected);
    }
    let mut notes = snapshot;
    notes.unknowns = vec![note("synthetic coverage note"); 8].try_into().unwrap();
    let no_rows = event_projection(notes, None);
    assert!(no_rows
        .series
        .iter()
        .all(|s| s.metric.channel() != TelemetryChannel::Processes));
    assert!(no_rows
        .candidates
        .iter()
        .all(|c| c.kind != TrendCandidateKind::Growth));
    assert_eq!(no_rows.unknowns.len(), 8);
    assert!(no_rows.unknowns[0].as_str().contains("snapshot"));
}

#[test]
fn the_candidate_cap_keeps_machine_candidates_before_many_growing_processes() {
    let mut h = History::default();
    for i in 0..4 {
        let mut r = raw();
        r.captured_at_unix_ms += i * 15000;
        r.processes_observed_at_unix_ms = Some(r.captured_at_unix_ms);
        r.processes = (0..70)
            .map(|pid| loomward_telemetry::Process {
                pid,
                private_commit_bytes: Some(i << 20),
                ..r.processes[0].clone()
            })
            .collect();
        r.memory = Observation::Observed {
            value: Memory {
                total_bytes: Some(1000),
                available_bytes: Some(50),
                standby_bytes: Some(300 - i * 10),
                ..Default::default()
            },
        };
        r.system_cpu_busy_fraction = Some(0.95);
        h.push(&r, i * 15000, 15000);
    }
    let report = h.report(CHANNELS);
    assert_eq!(report.candidates.len(), 64);
    assert!(report.candidates_truncated);
    let kinds: Vec<_> = report.candidates.iter().map(|c| c.kind).collect();
    assert!(kinds.contains(&TrendCandidateKind::MemoryPressure));
    assert!(kinds.contains(&TrendCandidateKind::CpuSaturation));
    // machine first, then disks, then processes; the cut falls on processes
    let channel = |c: &TrendCandidate| report.series[c.series_index.get() as usize].metric.channel();
    let order: Vec<_> = report.candidates.iter().map(channel).collect();
    assert!(order.windows(2).all(|w| {
        let rank = |c: &TelemetryChannel| match c {
            TelemetryChannel::System => 0,
            TelemetryChannel::Disks => 1,
            _ => 2,
        };
        rank(&w[0]) <= rank(&w[1])
    }));
    assert_eq!(
        report
            .candidates
            .iter()
            .filter(|c| c.kind == TrendCandidateKind::Growth)
            .count(),
        64 - kinds.iter().filter(|k| **k != TrendCandidateKind::Growth).count()
    );
}

#[test]
fn a_missing_value_only_ends_the_run_behind_it() {
    // null, then four good contiguous points: the newest run is a candidate
    let h = series(&[None, Some(0), Some(1 << 20), Some(2 << 20), Some(3 << 20)]);
    let c = growth(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].sample_count.get(), 4);
    assert_eq!(c[0].window_ms.get(), 45_000);
    assert_eq!(c[0].start_value, TrendValue::Bytes(0u64.into()));
    // a null after the run, or too few points behind it, is still no candidate
    assert!(growth(&series(&[Some(0), Some(1 << 20), Some(2 << 20), Some(3 << 20), None])).is_empty());
    assert!(growth(&series(&[Some(0), Some(1 << 20), None, Some(2 << 20), Some(3 << 20)])).is_empty());
    // a run never reaches back across the null for its fourth point
    let c = growth(&series(&[Some(0), None, Some(1 << 20), Some(2 << 20), Some(3 << 20), Some(4 << 20)]));
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].sample_count.get(), 4);
}

#[test]
fn memory_pressure_uses_the_newest_aligned_run_after_a_missing_sample() {
    let mut h = History::default();
    for i in 0..5 {
        let mut r = raw();
        r.captured_at_unix_ms += i * 1000;
        r.memory = Observation::Observed {
            value: Memory {
                total_bytes: Some(1000),
                available_bytes: if i == 0 { None } else { Some(50) },
                standby_bytes: Some(300 - i * 10),
                ..Memory::default()
            },
        };
        h.push(&r, i * 1000, 1000);
    }
    let report = h.report(CHANNELS);
    let c = report
        .candidates
        .iter()
        .find(|c| c.kind == TrendCandidateKind::MemoryPressure)
        .expect("the four samples after the null are a window");
    assert_eq!(c.sample_count.get(), 4);
    assert_eq!(c.window_ms.get(), 3000);
}
