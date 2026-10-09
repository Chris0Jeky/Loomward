//! Bounded local observations. No process control or admission authority.
use serde::{Deserialize, Serialize};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
mod windows;

pub const MAX_PROCESS_ROWS: usize = 1024;
pub const MAX_ENUMERATED_PROCESSES: usize = 8192;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Observation<T> {
    Observed { value: T },
    Unknown { reason: String },
    Unsupported { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unknown {
    pub field: String,
    pub reason: String,
    pub windows_error: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Memory {
    /// Physical RAM installed and usable by Windows, in bytes.
    pub physical_total_bytes: Option<u64>,
    /// RAM Windows can supply now, including reclaimable pages; not unused RAM.
    pub physical_available_bytes: Option<u64>,
    /// Committed allocations Windows has promised to back with RAM or page files.
    pub commit_charge_bytes: Option<u64>,
    /// Current ceiling of that promise, backed by RAM and page files.
    pub commit_limit_bytes: Option<u64>,
    /// Highest system commit charge observed since boot.
    pub commit_peak_bytes: Option<u64>,
    /// Resident system cache pages; overlaps other memory accounting categories.
    pub system_cache_bytes: Option<u64>,
    /// Kernel allocations that may be paged out; not all necessarily resident.
    pub kernel_paged_bytes: Option<u64>,
    /// Kernel allocations that must remain in physical RAM.
    pub kernel_nonpaged_bytes: Option<u64>,
    /// Size in bytes of one memory page used by performance counters.
    pub page_size_bytes: Option<u64>,
    pub unknowns: Vec<Unknown>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Rates {
    /// CPU time divided by elapsed time and all active logical processors.
    pub cpu_percent_of_machine: Option<f64>,
    pub io_read_bytes_per_second: Option<f64>,
    pub io_write_bytes_per_second: Option<f64>,
    pub io_other_bytes_per_second: Option<f64>,
    pub working_set_delta_bytes: Option<i64>,
    pub private_commit_delta_bytes: Option<i64>,
    pub unknowns: Vec<Unknown>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    /// Enumeration metadata, not proof of a live parent instance.
    pub parent_pid: u32,
    /// Basename from QueryFullProcessImageNameW; no executable paths exported.
    pub image_name: Option<String>,
    pub session_id: Option<u32>,
    /// Exact decimal Windows FILETIME ticks (100 ns since 1601), for PID reuse.
    pub start_time_windows_100ns: Option<String>,
    pub cpu_user_100ns: Option<u64>,
    pub cpu_kernel_100ns: Option<u64>,
    /// Resident pages, including shared pages; sums are not unique physical use.
    pub working_set_bytes: Option<u64>,
    /// Private committed allocations, not the resident working set.
    pub private_commit_bytes: Option<u64>,
    /// EX2-only private resident pages; null when EX2 is unavailable.
    pub private_working_set_bytes: Option<u64>,
    /// All process I/O, including non-disk devices; cumulative since start.
    pub io_read_bytes: Option<u64>,
    pub io_write_bytes: Option<u64>,
    pub io_other_bytes: Option<u64>,
    pub handle_count: Option<u32>,
    pub thread_count: u32,
    /// Any failed observation, including denial or exit; not a protection diagnosis.
    pub protected_or_unknown: bool,
    pub unknowns: Vec<Unknown>,
    pub rates: Rates,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProcessTotals {
    pub enumerated: usize,
    pub opened: usize,
    pub access_denied: usize,
    pub partial_or_unknown: usize,
    pub working_set_observed: usize,
    /// Sum over observed processes only. Shared pages may be counted repeatedly.
    pub working_set_sum_bytes: u64,
    pub private_commit_observed: usize,
    pub private_commit_sum_bytes: u64,
    pub enumeration_truncated: bool,
    pub enumeration_unknowns: Vec<Unknown>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiskIo {
    /// PDH physical-disk instance (may include multiple volume letters).
    pub instance: String,
    pub bytes_per_second: Option<f64>,
    /// Actual elapsed time between PDH collections, independent of process sampling.
    pub sample_interval_seconds: f64,
    pub unknown_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub status: String,
    pub platform: String,
    pub captured_at_unix_ms: u64,
    pub snapshot_cost_ms: f64,
    pub sample_interval_seconds: Option<f64>,
    pub logical_processor_count: Option<u32>,
    pub memory: Observation<Memory>,
    pub processes: Vec<Process>,
    pub process_totals: ProcessTotals,
    pub display_truncated: bool,
    pub gpu: Observation<()>,
    pub process_gpu: Observation<()>,
    pub disk_io: Observation<Vec<DiskIo>>,
}

pub fn snapshot(max_processes: usize) -> Snapshot {
    Sampler::default().sample(max_processes)
}

#[derive(Default)]
pub struct Sampler {
    previous: Option<(Instant, Snapshot)>,
    #[cfg(windows)]
    disk: windows::DiskSampler,
}

impl Sampler {
    pub fn sample(&mut self, max_processes: usize) -> Snapshot {
        let now = Instant::now();
        #[cfg(windows)]
        let mut current = windows::collect(&mut self.disk);
        #[cfg(not(windows))]
        let mut current = unsupported_snapshot();
        if let Some((previous_at, previous)) = &self.previous {
            let seconds = now.duration_since(*previous_at).as_secs_f64();
            current.sample_interval_seconds = Some(seconds);
            let prior: std::collections::HashMap<_, _> =
                previous.processes.iter().map(|p| (p.pid, p)).collect();
            for process in &mut current.processes {
                process.rates = match prior.get(&process.pid) {
                    Some(old) => derive_rates(
                        process,
                        old,
                        seconds,
                        current.logical_processor_count.unwrap_or(0),
                    ),
                    None => unknown_rates("no_previous_process_instance"),
                };
            }
        } else {
            for process in &mut current.processes {
                process.rates = unknown_rates("requires_two_samples");
            }
        }
        self.previous = Some((now, current.clone()));
        current
            .processes
            .sort_by_key(|p| (std::cmp::Reverse(p.working_set_bytes), p.pid));
        current.display_truncated = current.processes.len() > max_processes.min(MAX_PROCESS_ROWS);
        current
            .processes
            .truncate(max_processes.min(MAX_PROCESS_ROWS));
        current.snapshot_cost_ms = now.elapsed().as_secs_f64() * 1000.0;
        current
    }
}

fn unknown(field: &str, reason: &str) -> Unknown {
    Unknown {
        field: field.into(),
        reason: reason.into(),
        windows_error: None,
    }
}

fn unknown_rates(reason: &str) -> Rates {
    Rates {
        unknowns: vec![unknown("all_rates", reason)],
        ..Rates::default()
    }
}

fn derive_rates(current: &Process, previous: &Process, seconds: f64, cores: u32) -> Rates {
    if current.pid != previous.pid
        || current.start_time_windows_100ns.is_none()
        || current.start_time_windows_100ns != previous.start_time_windows_100ns
    {
        return unknown_rates("process_instance_not_matched");
    }
    if !seconds.is_finite() || seconds <= 0.0 {
        return unknown_rates("invalid_sample_interval");
    }
    let mut rates = Rates::default();
    let cpu_delta = current
        .cpu_user_100ns
        .zip(previous.cpu_user_100ns)
        .and_then(|(a, b)| a.checked_sub(b))
        .zip(
            current
                .cpu_kernel_100ns
                .zip(previous.cpu_kernel_100ns)
                .and_then(|(a, b)| a.checked_sub(b)),
        )
        .and_then(|(a, b)| a.checked_add(b));
    rates.cpu_percent_of_machine = cpu_delta
        .filter(|_| cores > 0)
        .map(|delta| delta as f64 / 10_000_000.0 / seconds / cores as f64 * 100.0)
        .filter(|v| v.is_finite());
    if rates.cpu_percent_of_machine.is_none() {
        rates.unknowns.push(unknown(
            "cpu_percent_of_machine",
            "missing_or_regressed_counter_or_unknown_cpu_capacity",
        ));
    }
    for (field, current, previous, output) in [
        (
            "io_read_bytes_per_second",
            current.io_read_bytes,
            previous.io_read_bytes,
            &mut rates.io_read_bytes_per_second,
        ),
        (
            "io_write_bytes_per_second",
            current.io_write_bytes,
            previous.io_write_bytes,
            &mut rates.io_write_bytes_per_second,
        ),
        (
            "io_other_bytes_per_second",
            current.io_other_bytes,
            previous.io_other_bytes,
            &mut rates.io_other_bytes_per_second,
        ),
    ] {
        *output = current
            .zip(previous)
            .and_then(|(a, b)| a.checked_sub(b))
            .map(|delta| delta as f64 / seconds)
            .filter(|v| v.is_finite());
        if output.is_none() {
            rates
                .unknowns
                .push(unknown(field, "missing_or_regressed_counter"));
        }
    }
    for (field, current, previous, output) in [
        (
            "working_set_delta_bytes",
            current.working_set_bytes,
            previous.working_set_bytes,
            &mut rates.working_set_delta_bytes,
        ),
        (
            "private_commit_delta_bytes",
            current.private_commit_bytes,
            previous.private_commit_bytes,
            &mut rates.private_commit_delta_bytes,
        ),
    ] {
        *output = current
            .zip(previous)
            .and_then(|(a, b)| i64::try_from(a as i128 - b as i128).ok());
        if output.is_none() {
            rates
                .unknowns
                .push(unknown(field, "missing_counter_or_delta_out_of_range"));
        }
    }
    rates
}

fn unsupported_snapshot() -> Snapshot {
    let reason = "native telemetry is implemented only on Windows".to_owned();
    Snapshot {
        schema_version: 1,
        status: "unsupported".into(),
        platform: std::env::consts::OS.into(),
        captured_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        snapshot_cost_ms: 0.0,
        sample_interval_seconds: None,
        logical_processor_count: None,
        memory: Observation::Unsupported {
            reason: reason.clone(),
        },
        processes: vec![],
        process_totals: ProcessTotals::default(),
        display_truncated: false,
        gpu: Observation::Unsupported {
            reason: reason.clone(),
        },
        process_gpu: Observation::Unsupported {
            reason: reason.clone(),
        },
        disk_io: Observation::Unsupported { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process() -> Process {
        Process {
            pid: 7,
            start_time_windows_100ns: Some("123".into()),
            cpu_user_100ns: Some(0),
            cpu_kernel_100ns: Some(0),
            io_read_bytes: Some(100),
            io_write_bytes: Some(0),
            io_other_bytes: Some(0),
            working_set_bytes: Some(1000),
            private_commit_bytes: Some(500),
            ..Process::default()
        }
    }

    #[test]
    fn known_two_sample_rates_and_signed_memory_deltas() {
        let previous = process();
        let current = Process {
            cpu_user_100ns: Some(10_000_000),
            io_read_bytes: Some(300),
            working_set_bytes: Some(900),
            private_commit_bytes: Some(600),
            ..previous.clone()
        };
        let rates = derive_rates(&current, &previous, 2.0, 4);
        assert_eq!(rates.cpu_percent_of_machine, Some(12.5));
        assert_eq!(rates.io_read_bytes_per_second, Some(100.0));
        assert_eq!(rates.working_set_delta_bytes, Some(-100));
        assert_eq!(rates.private_commit_delta_bytes, Some(100));
        assert!(rates.unknowns.is_empty());
    }

    #[test]
    fn pid_reuse_missing_identity_and_invalid_windows_have_no_rates() {
        let previous = process();
        for current in [
            Process {
                start_time_windows_100ns: Some("456".into()),
                ..previous.clone()
            },
            Process {
                start_time_windows_100ns: None,
                ..previous.clone()
            },
            Process {
                pid: 8,
                ..previous.clone()
            },
        ] {
            let rates = derive_rates(&current, &previous, 1.0, 1);
            assert!(rates.cpu_percent_of_machine.is_none());
            assert!(rates.working_set_delta_bytes.is_none());
            assert!(!rates.unknowns.is_empty());
        }
        for seconds in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(!derive_rates(&previous, &previous, seconds, 1)
                .unknowns
                .is_empty());
        }
    }

    #[test]
    fn regressed_or_missing_counters_remain_unknown() {
        let previous = process();
        let current = Process {
            io_read_bytes: Some(50),
            cpu_user_100ns: None,
            working_set_bytes: None,
            ..previous.clone()
        };
        let rates = derive_rates(&current, &previous, 1.0, 1);
        assert!(rates.io_read_bytes_per_second.is_none());
        assert!(rates.cpu_percent_of_machine.is_none());
        assert!(rates.working_set_delta_bytes.is_none());
        assert_eq!(rates.unknowns.len(), 3);
    }
}
