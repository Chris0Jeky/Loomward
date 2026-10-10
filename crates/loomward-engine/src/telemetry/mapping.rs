//! Native schema v2 to the exact v3 wire quantities; missing values stay null.
use crate::{EngineError, EngineResult};
use loomward_protocol::*;
use loomward_telemetry::{Observation, Process, Snapshot};

pub(super) fn invalid(error: Invalid) -> EngineError {
    EngineError::Internal {
        message: format!("telemetry contract: {error}"),
        detail: None,
    }
}

// Same civil_from_days conversion as the HTTP fixture clock, retaining FILETIME's 100 ns.
pub(super) fn timestamp(unix_ticks: i128) -> EngineResult<Timestamp> {
    let seconds = unix_ticks.div_euclid(10_000_000);
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i128::from(month <= 2);
    Timestamp::new(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:07}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        unix_ticks.rem_euclid(10_000_000)
    ))
    .map_err(invalid)
}

pub(super) fn observed_at(raw: &Snapshot) -> EngineResult<Timestamp> {
    timestamp(i128::from(raw.captured_at_unix_ms) * 10_000)
}

pub(super) fn process_ref(process: &Process, sequence: u64) -> ProcessRef {
    // Unknown creation times are sample-scoped: a PID alone must never reconnect two instances.
    let instance = process
        .start_time_windows_100ns
        .as_deref()
        .filter(|s| s.len() <= 20 && s.bytes().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("unknown_{sequence}"));
    ProcessRef::new(format!("pc_{}_{}", process.pid, instance))
        .expect("bounded decimal observation identity")
}

pub(super) fn process_row(process: &Process, sequence: u64) -> ProcessRow {
    let started_at = process
        .start_time_windows_100ns
        .as_deref()
        .and_then(|v| v.parse::<u64>().ok())
        .and_then(|v| timestamp(i128::from(v) - 116_444_736_000_000_000).ok());
    let denied = process
        .unknowns
        .iter()
        .any(|u| u.field == "process_handle_fields" && u.reason == "access_denied");
    let access = if denied {
        ProcessRowAccess::Denied
    } else if process.protected_or_unknown || started_at.is_none() {
        ProcessRowAccess::Limited
    } else {
        ProcessRowAccess::Full
    };
    ProcessRow {
        process_ref: process_ref(process, sequence),
        pid: Int::new(i64::from(process.pid)).expect("u32 pid"),
        name: Text::truncated(process.image_name.as_deref().unwrap_or("Unknown process")),
        started_at,
        private_commit_bytes: process.private_commit_bytes.map(Bytes::from),
        private_working_set_bytes: process.private_working_set_bytes.map(Bytes::from),
        working_set_bytes: process.working_set_bytes.map(Bytes::from),
        cpu_fraction: process
            .rates
            .cpu_fraction
            .and_then(|v| Fraction::new(v).ok()),
        io_read_bytes_per_s: process
            .rates
            .io_read_bytes_per_s
            .and_then(|v| Rate::new(v).ok()),
        io_write_bytes_per_s: process
            .rates
            .io_write_bytes_per_s
            .and_then(|v| Rate::new(v).ok()),
        gpu_dedicated_bytes: None,
        access,
        loomward_owned: process.pid == std::process::id(),
    }
}

pub(super) fn sorted_rows(
    raw: &Snapshot,
    sequence: u64,
    sort: ProcessListRequestSort,
) -> Vec<ProcessRow> {
    let mut rows: Vec<_> = raw
        .processes
        .iter()
        .map(|p| process_row(p, sequence))
        .collect();
    rows.sort_by(|a, b| {
        let ordering = match sort {
            ProcessListRequestSort::PrivateDesc => {
                b.private_commit_bytes.cmp(&a.private_commit_bytes)
            }
            ProcessListRequestSort::WorkingSetDesc => b.working_set_bytes.cmp(&a.working_set_bytes),
            ProcessListRequestSort::CpuDesc => b
                .cpu_fraction
                .partial_cmp(&a.cpu_fraction)
                .unwrap_or(std::cmp::Ordering::Equal),
            ProcessListRequestSort::IoDesc => {
                let total = |p: &ProcessRow| {
                    p.io_read_bytes_per_s
                        .zip(p.io_write_bytes_per_s)
                        .map(|(r, w)| r.get() + w.get())
                };
                total(b)
                    .partial_cmp(&total(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
            ProcessListRequestSort::GpuDesc => b.gpu_dedicated_bytes.cmp(&a.gpu_dedicated_bytes),
            ProcessListRequestSort::NameAsc => a
                .name
                .as_str()
                .to_lowercase()
                .cmp(&b.name.as_str().to_lowercase()),
        };
        ordering.then_with(|| a.pid.cmp(&b.pid))
    });
    rows
}

pub(super) fn health(own: Option<&Process>) -> HealthEngine {
    HealthEngine {
        private_commit_bytes: own.and_then(|p| p.private_commit_bytes).map(Bytes::from),
        working_set_bytes: own.and_then(|p| p.working_set_bytes).map(Bytes::from),
        cpu_seconds: own
            .and_then(|p| p.cpu_user_100ns.zip(p.cpu_kernel_100ns))
            .and_then(|(u, k)| u.checked_add(k))
            .and_then(|v| Rate::new(v as f64 / 10_000_000.0).ok()),
        threads: own.map(|p| Count::from(p.thread_count)),
    }
}

pub(super) fn sample(
    raw: &Snapshot,
    own: Option<&Process>,
    sequence: u64,
    channels: &[TelemetryChannel],
    busy: bool,
) -> EngineResult<TelemetrySample> {
    let requested = |channel| channels.contains(&channel);
    let system = if requested(TelemetryChannel::System) {
        raw.logical_processor_count.filter(|v| *v > 0).map(|cpus| {
            let memory = match &raw.memory {
                Observation::Observed { value } => Some(value),
                _ => None,
            };
            SystemSample {
                memory: SystemSampleMemory {
                    total_bytes: memory.and_then(|v| v.total_bytes).map(Bytes::from),
                    available_bytes: memory.and_then(|v| v.available_bytes).map(Bytes::from),
                    commit_bytes: memory.and_then(|v| v.commit_bytes).map(Bytes::from),
                    commit_limit_bytes: memory.and_then(|v| v.commit_limit_bytes).map(Bytes::from),
                    load_fraction: memory
                        .and_then(|v| v.load_fraction)
                        .and_then(|v| Fraction::new(v).ok()),
                },
                cpu: SystemSampleCpu {
                    logical_cpus: cpus.into(),
                    busy_fraction: raw
                        .system_cpu_busy_fraction
                        .and_then(|v| Fraction::new(v).ok()),
                },
            }
        })
    } else {
        None
    };
    let gpu = if requested(TelemetryChannel::Gpu) {
        let (state, basis, adapters) = match &raw.gpu {
            Observation::Observed { value } => {
                let adapters = value
                    .iter()
                    .take(8)
                    .map(|a| {
                        Ok(GpuSampleAdapter {
                            adapter_id: Text::new(&a.adapter_id).map_err(invalid)?,
                            name: Text::truncated("Unknown adapter name"),
                            dedicated_total_bytes: a.dedicated_total_bytes.map(Bytes::from),
                            dedicated_used_bytes: a.dedicated_used_bytes.map(Bytes::from),
                            shared_used_bytes: a.shared_used_bytes.map(Bytes::from),
                            engine_busy_fraction: a
                                .engine_busy_fraction
                                .and_then(|v| Fraction::new(v).ok()),
                        })
                    })
                    .collect::<EngineResult<Vec<_>>>()?;
                (
                    GpuSampleState::Observed,
                    GpuSampleBasis::PdhGpuCounters,
                    adapters,
                )
            }
            _ => (
                GpuSampleState::Unavailable,
                GpuSampleBasis::Unavailable,
                vec![],
            ),
        };
        Some(GpuSample {
            state,
            basis,
            adapters: adapters.try_into().map_err(invalid)?,
        })
    } else {
        None
    };
    let disks = if requested(TelemetryChannel::Disks) {
        let (state, disks) = match &raw.disk_io {
            Observation::Observed { value } => (
                DiskSampleState::Observed,
                value
                    .iter()
                    .take(16)
                    .map(|d| DiskSampleDisk {
                        disk_label: Text::truncated(&d.disk_label),
                        volume_ids: BoundedVec::default(), // No catalogue volume mapping exists in the sampler.
                        read_bytes_per_s: d.read_bytes_per_s.and_then(|v| Rate::new(v).ok()),
                        write_bytes_per_s: d.write_bytes_per_s.and_then(|v| Rate::new(v).ok()),
                        busy_fraction: d.busy_fraction.and_then(|v| Fraction::new(v).ok()),
                        queue_length: d.queue_length.and_then(|v| Rate::new(v).ok()),
                    })
                    .collect(),
            ),
            _ => (DiskSampleState::Unavailable, vec![]),
        };
        Some(DiskSample {
            state,
            disks: disks.try_into().map_err(invalid)?,
        })
    } else {
        None
    };
    let engine = if requested(TelemetryChannel::Engine) {
        let health = health(own);
        Some(EngineSample {
            private_commit_bytes: health.private_commit_bytes,
            working_set_bytes: health.working_set_bytes,
            threads: health.threads,
            cpu_fraction: own
                .and_then(|p| p.rates.cpu_fraction)
                .and_then(|v| Fraction::new(v).ok()),
            pools: vec![PoolUse {
                pool: PoolName::Telemetry,
                max_workers: Int::new(1).unwrap(),
                busy_workers: Int::new(i64::from(busy)).unwrap(),
                queue_depth: Count::ZERO,
                queue_capacity: Count::ZERO,
            }]
            .try_into()
            .map_err(invalid)?,
        })
    } else {
        None
    };
    let processes = if requested(TelemetryChannel::Processes)
        && raw.status != "unsupported"
        && raw.process_totals.enumeration_unknowns.is_empty()
    {
        let mut rows = sorted_rows(raw, sequence, ProcessListRequestSort::CpuDesc);
        rows.truncate(20);
        Some(ProcessSummary {
            observed_count: Count::saturating(raw.process_totals.enumerated as u64),
            denied_count: Count::saturating(raw.process_totals.access_denied as u64),
            top: rows.try_into().map_err(invalid)?,
        })
    } else {
        None
    };
    Ok(TelemetrySample {
        sample_seq: Count::saturating(sequence),
        observed_at: observed_at(raw)?,
        elapsed_ms: raw
            .sample_interval_seconds
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| Count::saturating((v * 1000.0).round() as u64)),
        system,
        gpu,
        disks,
        engine,
        processes,
    })
}
