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
    pub total_bytes: Option<u64>,
    /// RAM Windows can supply now, including reclaimable pages; not unused RAM.
    pub available_bytes: Option<u64>,
    /// Committed allocations Windows has promised to back with RAM or page files.
    pub commit_bytes: Option<u64>,
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
    /// Standby lists (core + normal priority + reserve); reclaimable, not free.
    pub standby_bytes: Option<u64>,
    /// Dirty pages waiting for writeback.
    pub modified_bytes: Option<u64>,
    /// Free and zeroed pages, excluding standby.
    pub free_bytes: Option<u64>,
    /// GlobalMemoryStatusEx physical memory load, as a fraction.
    pub load_fraction: Option<f64>,
    pub unknowns: Vec<Unknown>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Rates {
    /// (user + kernel) CPU seconds / (monotonic interval * all active logical CPUs).
    pub cpu_fraction: Option<f64>,
    pub io_read_bytes_per_s: Option<f64>,
    pub io_write_bytes_per_s: Option<f64>,
    pub io_other_bytes_per_s: Option<f64>,
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
    pub thread_count: Option<u32>,
    /// Any failed observation, including denial or exit; not a protection diagnosis.
    pub protected_or_unknown: bool,
    pub unknowns: Vec<Unknown>,
    pub rates: Rates,
}

impl Process {
    /// Access denied while opening the process, not while querying an individual field.
    pub fn access_denied(&self) -> bool {
        self.unknowns.iter().any(|u| {
            u.field == "process_handle_fields"
                && (u.reason == "access_denied" || u.windows_error == Some(5))
        })
    }
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
    /// Overflowed sums are saturated lower bounds, not exact byte totals.
    #[serde(default)]
    pub sum_unknowns: Vec<Unknown>,
    pub enumeration_truncated: bool,
    pub enumeration_unknowns: Vec<Unknown>,
}

#[cfg(any(windows, test))]
impl ProcessTotals {
    fn observe(&mut self, row: &Process, opened: bool) {
        self.opened += usize::from(opened);
        self.access_denied += usize::from(row.access_denied());
        self.partial_or_unknown += usize::from(row.protected_or_unknown);
        for (field, bytes, observed, sum) in [
            (
                "working_set_sum_bytes",
                row.working_set_bytes,
                &mut self.working_set_observed,
                &mut self.working_set_sum_bytes,
            ),
            (
                "private_commit_sum_bytes",
                row.private_commit_bytes,
                &mut self.private_commit_observed,
                &mut self.private_commit_sum_bytes,
            ),
        ] {
            if let Some(bytes) = bytes {
                *observed += 1;
                *sum = sum.checked_add(bytes).unwrap_or_else(|| {
                    if !self.sum_unknowns.iter().any(|u| u.field == field) {
                        self.sum_unknowns.push(unknown(
                            field,
                            "sum_overflow; reported_bytes_are_a_lower_bound",
                        ));
                    }
                    u64::MAX
                });
            }
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiskIo {
    /// PDH physical-disk instance (may include multiple volume letters).
    pub disk_label: String,
    pub read_bytes_per_s: Option<f64>,
    pub write_bytes_per_s: Option<f64>,
    /// 1 - PhysicalDisk % Idle Time / 100, clamped to [0, 1].
    pub busy_fraction: Option<f64>,
    /// PhysicalDisk Avg. Disk Queue Length, not utilisation.
    pub queue_length: Option<f64>,
    /// PDH collection interval; null before two successful collections.
    pub sample_interval_seconds: Option<f64>,
    pub unknowns: Vec<Unknown>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GpuEngine {
    pub engine_type: Option<String>,
    /// Sum over this adapter's instances of this type, clamped to [0, 1].
    pub busy_fraction: Option<f64>,
    pub unknowns: Vec<Unknown>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GpuAdapter {
    /// LUID from PDH; an observation key, not a grant or a device name.
    pub adapter_id: String,
    pub dedicated_total_bytes: Option<u64>,
    /// Global GPU Adapter Memory counters, never sums of process memory.
    pub dedicated_used_bytes: Option<u64>,
    pub shared_used_bytes: Option<u64>,
    /// Maximum over aggregated engine types, clamped to [0, 1].
    pub engine_busy_fraction: Option<f64>,
    pub engines: Vec<GpuEngine>,
    pub unknowns: Vec<Unknown>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub status: String,
    pub platform: String,
    pub captured_at_unix_ms: u64,
    /// Time of the retained process enumeration, independent of system counters.
    pub processes_observed_at_unix_ms: Option<u64>,
    pub sampled_channels: SampleChannels,
    pub snapshot_cost_ms: f64,
    pub sample_interval_seconds: Option<f64>,
    pub logical_processor_count: Option<u32>,
    pub memory: Observation<Memory>,
    pub processes: Vec<Process>,
    pub process_totals: ProcessTotals,
    pub display_truncated: bool,
    pub system_cpu_busy_fraction: Option<f64>,
    pub gpu: Observation<Vec<GpuAdapter>>,
    pub process_gpu: Observation<()>,
    pub disk_io: Observation<Vec<DiskIo>>,
    /// Skipped malformed array names across all counters in this sample.
    pub pdh_malformed_instances: usize,
    pub pdh_unknowns: Vec<Unknown>,
}

pub fn snapshot(max_processes: usize) -> Snapshot {
    Sampler::default().sample(max_processes)
}

/// Own-process aggregate counters for P12/P13; no process names or paths.
#[derive(Clone, Debug)]
pub struct OwnUsage {
    /// Cumulative user + kernel CPU seconds over all process threads.
    pub cpu_seconds: f64,
    /// Cumulative CPU seconds for the calling thread.
    pub calling_thread_cpu_seconds: f64,
    /// Current private committed allocations (PrivateUsage).
    pub private_commit_bytes: u64,
    /// True high-water private commit (PeakPagefileUsage).
    pub peak_private_commit_bytes: u64,
    /// Resident pages including shared pages, observed directly.
    pub working_set_bytes: u64,
}

/// The host facts a diagnostic bundle may carry (owner decision q-16): the OS build number, the
/// active logical processor count and installed physical memory. `None` when the API failed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Host {
    pub os_build: Option<u32>,
    pub logical_processors: Option<u32>,
    pub memory_total_bytes: Option<u64>,
}

pub fn host() -> Host {
    #[cfg(windows)]
    {
        windows::host()
    }
    #[cfg(not(windows))]
    {
        Host::default()
    }
}

/// Read current own CPU, private commit and working set. Failed APIs make the observation unknown.
pub fn own_usage() -> Observation<OwnUsage> {
    #[cfg(windows)]
    {
        windows::own_usage()
    }
    #[cfg(not(windows))]
    {
        Observation::Unsupported {
            reason: "native own-usage measurement requires Windows".into(),
        }
    }
}

/// Native work requested by the current lease union.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleChannels {
    pub system: bool,
    pub processes: bool,
    pub gpu: bool,
    pub disks: bool,
    pub engine: bool,
}

impl SampleChannels {
    pub const ALL: Self = Self {
        system: true,
        processes: true,
        gpu: true,
        disks: true,
        engine: true,
    };
}

/// Aggregate wall-clock phase costs; no names, paths or counter values.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SamplingCosts {
    pub samples: u64,
    pub process_enumeration_ms: f64,
    pub process_snapshot_ms: f64,
    pub process_snapshots: u64,
    pub process_queries_ms: f64,
    pub process_walk_ms: f64,
    pub system_counters_ms: f64,
    pub pdh_collection_ms: f64,
    pub wildcard_expansion_ms: f64,
    pub wildcard_expansions: u64,
    pub pdh_decode_ms: f64,
    /// GPU array decoding/aggregation, excluding expansion; a subset of pdh_decode_ms.
    pub gpu_instances_ms: f64,
    pub native_other_ms: f64,
    pub rates_and_projection_ms: f64,
    pub engine_mapping_ms: f64,
    pub engine_snapshot_mapping_ms: f64,
    pub engine_publication_ms: f64,
    pub engine_trends_ms: f64,
    pub engine_snapshot_trends_ms: f64,
    pub mapping_errors: u64,
}

impl SamplingCosts {
    pub fn add(&mut self, other: &Self) {
        self.samples += other.samples;
        self.process_enumeration_ms += other.process_enumeration_ms;
        self.process_snapshot_ms += other.process_snapshot_ms;
        self.process_snapshots += other.process_snapshots;
        self.process_queries_ms += other.process_queries_ms;
        self.process_walk_ms += other.process_walk_ms;
        self.system_counters_ms += other.system_counters_ms;
        self.pdh_collection_ms += other.pdh_collection_ms;
        self.wildcard_expansion_ms += other.wildcard_expansion_ms;
        self.wildcard_expansions += other.wildcard_expansions;
        self.pdh_decode_ms += other.pdh_decode_ms;
        self.gpu_instances_ms += other.gpu_instances_ms;
        self.native_other_ms += other.native_other_ms;
        self.rates_and_projection_ms += other.rates_and_projection_ms;
        self.engine_mapping_ms += other.engine_mapping_ms;
        self.engine_snapshot_mapping_ms += other.engine_snapshot_mapping_ms;
        self.engine_publication_ms += other.engine_publication_ms;
        self.engine_trends_ms += other.engine_trends_ms;
        self.engine_snapshot_trends_ms += other.engine_snapshot_trends_ms;
        self.mapping_errors += other.mapping_errors;
    }
}

#[derive(Default)]
pub struct Sampler {
    previous: Option<(Instant, Snapshot)>,
    costs: SamplingCosts,
    channels: Option<SampleChannels>,
    process_sample_at: Option<Instant>,
    /// The engine's own row and when its counters were read, which is not the sample start: a
    /// cadence sample reads it straight after PDH, a refresh somewhere inside the process walk.
    own: Option<(Instant, Process)>,
    #[cfg(windows)]
    pdh: windows::PdhSampler,
    #[cfg(windows)]
    image_name_buffer: Vec<u16>,
}

impl Sampler {
    pub fn phase_costs(&self) -> &SamplingCosts {
        &self.costs
    }

    /// The engine's own row before display truncation; no new OS observation is performed.
    ///
    /// Its CPU rate divides by the gap between the two counter reads. On a refresh sample the own
    /// row inside `Snapshot::processes` is rated by the 15 s walk-to-walk window like every other
    /// process, so the two can differ; the engine reads this one.
    pub fn own_process(&self) -> Option<&Process> {
        self.own.as_ref().map(|(_, own)| own)
    }

    pub fn sample(&mut self, max_processes: usize) -> Snapshot {
        self.sample_channels(max_processes, SampleChannels::ALL)
    }

    pub fn sample_channels(&mut self, max_processes: usize, channels: SampleChannels) -> Snapshot {
        self.sample_with_process_interval(max_processes, channels, std::time::Duration::ZERO)
    }

    /// Leases refresh process metadata at most every fifteen seconds; system/own counters keep their cadence.
    pub fn sample_leased(&mut self, max_processes: usize, channels: SampleChannels) -> Snapshot {
        self.sample_with_process_interval(
            max_processes,
            channels,
            std::time::Duration::from_secs(15),
        )
    }

    fn sample_with_process_interval(
        &mut self,
        max_processes: usize,
        channels: SampleChannels,
        process_interval: std::time::Duration,
    ) -> Snapshot {
        if self.channels != Some(channels) {
            #[cfg(windows)]
            {
                self.pdh = windows::PdhSampler::default();
            }
            self.channels = Some(channels);
            self.previous = None;
            self.process_sample_at = None;
            self.own = None;
        }
        let now = Instant::now();
        self.costs = SamplingCosts {
            samples: 1,
            ..SamplingCosts::default()
        };
        let refresh_processes = self
            .process_sample_at
            .is_none_or(|at| now.duration_since(at) >= process_interval);
        #[allow(unused_mut)] // only the Windows collector reads the own row
        let mut own_read_at = None;
        #[cfg(windows)]
        let mut current = windows::collect(
            &mut self.pdh,
            &mut self.costs,
            channels,
            refresh_processes,
            &mut self.image_name_buffer,
            &mut own_read_at,
        );
        #[cfg(not(windows))]
        let mut current = unsupported_snapshot();
        current.sampled_channels = channels;
        let native_ms = now.elapsed().as_secs_f64() * 1000.0;
        self.costs.native_other_ms = (native_ms
            - self.costs.system_counters_ms
            - self.costs.process_enumeration_ms
            - self.costs.pdh_collection_ms
            - self.costs.wildcard_expansion_ms
            - self.costs.pdh_decode_ms)
            .max(0.0);
        self.finish_sample(
            current,
            max_processes,
            channels,
            refresh_processes,
            now,
            own_read_at,
        )
    }

    fn finish_sample(
        &mut self,
        mut current: Snapshot,
        max_processes: usize,
        channels: SampleChannels,
        refresh_processes: bool,
        now: Instant,
        own_read_at: Option<Instant>,
    ) -> Snapshot {
        let rates_started = Instant::now();
        let mut own = current
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .cloned();
        if !refresh_processes {
            // A leased own row is built from a fabricated entry, so its parent comes from the last walk.
            let walked = self
                .previous
                .as_ref()
                .filter(|(_, previous)| previous.processes_observed_at_unix_ms.is_some())
                .and_then(|(_, previous)| {
                    previous
                        .processes
                        .iter()
                        .find(|p| p.pid == std::process::id())
                });
            if let (Some(own), Some(walked)) = (&mut own, walked) {
                own.parent_pid = walked.parent_pid;
            }
        }
        // One instant for both the rate and the stored reference, so the two can never disagree.
        let read_at = own_read_instant(own_read_at, now);
        if let Some((previous_at, previous)) = &self.previous {
            let seconds = now.duration_since(*previous_at).as_secs_f64();
            current.sample_interval_seconds = Some(seconds);
            if let Some(own) = &mut own {
                // #446: divide by the time between the two counter reads. Start-to-start time
                // undercounts by the process walk whenever a refresh follows a cadence sample.
                own.rates = own_rates(
                    own,
                    self.own.as_ref(),
                    read_at,
                    current.logical_processor_count.unwrap_or(0),
                );
            }
            if refresh_processes {
                let process_seconds = self
                    .process_sample_at
                    .map_or(seconds, |at| now.duration_since(at).as_secs_f64());
                let prior: std::collections::HashMap<_, _> =
                    previous.processes.iter().map(|p| (p.pid, p)).collect();
                for process in &mut current.processes {
                    process.rates = prior.get(&process.pid).map_or_else(
                        || unknown_rates("no_previous_process_instance"),
                        |old| {
                            derive_rates(
                                process,
                                old,
                                process_seconds,
                                current.logical_processor_count.unwrap_or(0),
                            )
                        },
                    );
                }
            } else if channels.processes {
                current.processes = previous.processes.clone();
                current.process_totals = previous.process_totals.clone();
                current.processes_observed_at_unix_ms = previous.processes_observed_at_unix_ms;
                if current.process_totals.partial_or_unknown > 0
                    || current.process_totals.enumeration_truncated
                    || !current.process_totals.enumeration_unknowns.is_empty()
                    || !current.process_totals.sum_unknowns.is_empty()
                {
                    current.status = "partial".into();
                }
            }
        } else {
            for process in &mut current.processes {
                process.rates = unknown_rates("requires_two_samples");
            }
            if let Some(own) = &mut own {
                own.rates = unknown_rates("requires_two_samples");
            }
        }
        if refresh_processes {
            self.process_sample_at = Some(now);
        }
        // Process-only cached ticks made no own-counter read; retain its original observation.
        if refresh_processes || channels.engine || !channels.processes {
            self.own = own.map(|own| (read_at, own));
        }
        self.previous = Some((now, current.clone()));
        current
            .processes
            .sort_by_key(|p| (std::cmp::Reverse(p.working_set_bytes), p.pid));
        current.display_truncated = current.processes.len() > max_processes.min(MAX_PROCESS_ROWS);
        current
            .processes
            .truncate(max_processes.min(MAX_PROCESS_ROWS));
        if !channels.processes {
            current.processes.clear();
        }
        self.costs.rates_and_projection_ms = rates_started.elapsed().as_secs_f64() * 1000.0;
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

/// The instant an own-row CPU reading was taken: its counter read when the collector made one,
/// otherwise the sample start. The current rate and the stored previous read both use it, so every
/// own rate divides by the gap between two counter reads (#446, #469).
fn own_read_instant(own_read_at: Option<Instant>, sample_started: Instant) -> Instant {
    own_read_at.unwrap_or(sample_started)
}

/// Own-row rate over the gap between the previous and current counter reads (#446, #467). Pure,
/// so the denominator can be pinned without sampling the OS.
fn own_rates(
    own: &Process,
    previous: Option<&(Instant, Process)>,
    read_at: Instant,
    cores: u32,
) -> Rates {
    previous.map_or_else(
        || unknown_rates("requires_two_samples"),
        |(old_at, old)| {
            derive_rates(
                own,
                old,
                read_at.duration_since(*old_at).as_secs_f64(),
                cores,
            )
        },
    )
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
    rates.cpu_fraction = cpu_delta
        .filter(|_| cores > 0)
        .map(|delta| delta as f64 / 10_000_000.0 / seconds / cores as f64)
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v));
    if rates.cpu_fraction.is_none() {
        rates.unknowns.push(unknown(
            "cpu_fraction",
            "missing_or_regressed_counter_unknown_cpu_capacity_or_fraction_out_of_range",
        ));
    }
    for (field, current, previous, output) in [
        (
            "io_read_bytes_per_s",
            current.io_read_bytes,
            previous.io_read_bytes,
            &mut rates.io_read_bytes_per_s,
        ),
        (
            "io_write_bytes_per_s",
            current.io_write_bytes,
            previous.io_write_bytes,
            &mut rates.io_write_bytes_per_s,
        ),
        (
            "io_other_bytes_per_s",
            current.io_other_bytes,
            previous.io_other_bytes,
            &mut rates.io_other_bytes_per_s,
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
        schema_version: 2,
        status: "unsupported".into(),
        platform: std::env::consts::OS.into(),
        processes_observed_at_unix_ms: None,
        sampled_channels: SampleChannels::ALL,
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
        system_cpu_busy_fraction: None,
        pdh_malformed_instances: 0,
        pdh_unknowns: vec![],
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

    #[test]
    fn process_totals_count_only_handle_access_denials() {
        let mut totals = ProcessTotals::default();
        for field in [
            "io_counters",
            "handle_count",
            "session_id",
            "private_working_set_bytes",
        ] {
            let row = Process {
                working_set_bytes: Some(200),
                private_commit_bytes: Some(100),
                protected_or_unknown: true,
                unknowns: vec![Unknown {
                    field: field.into(),
                    reason: "access_denied".into(),
                    windows_error: Some(5),
                }],
                ..Process::default()
            };
            totals.observe(&row, true);
            assert_eq!(totals.access_denied, 0);
        }
        let row = Process {
            protected_or_unknown: true,
            unknowns: vec![Unknown {
                field: "process_handle_fields".into(),
                reason: "access_denied".into(),
                windows_error: Some(5),
            }],
            ..Process::default()
        };
        totals.observe(&row, false);
        assert_eq!(totals.access_denied, 1);
        assert_eq!(totals.opened, 4);
        assert_eq!(totals.working_set_observed, 4);
        assert_eq!(totals.working_set_sum_bytes, 800);
    }

    #[test]
    fn process_sum_overflow_is_a_visible_lower_bound_and_does_not_hide_rows() {
        let mut ordinary = ProcessTotals::default();
        for bytes in [7, 9] {
            ordinary.observe(
                &Process {
                    working_set_bytes: Some(bytes),
                    private_commit_bytes: Some(bytes),
                    ..Process::default()
                },
                true,
            );
        }
        assert_eq!(ordinary.working_set_sum_bytes, 16);
        assert_eq!(ordinary.private_commit_sum_bytes, 16);
        assert!(ordinary.sum_unknowns.is_empty());
        let mut legacy = serde_json::to_value(&ordinary).unwrap();
        legacy.as_object_mut().unwrap().remove("sum_unknowns");
        assert!(serde_json::from_value::<ProcessTotals>(legacy)
            .unwrap()
            .sum_unknowns
            .is_empty());
        let mut totals = ProcessTotals::default();
        for bytes in [u64::MAX, 1, 2] {
            totals.observe(
                &Process {
                    working_set_bytes: Some(bytes),
                    private_commit_bytes: Some(bytes),
                    ..Process::default()
                },
                true,
            );
        }
        assert_eq!(totals.working_set_sum_bytes, u64::MAX);
        assert_eq!(totals.private_commit_sum_bytes, u64::MAX);
        assert_eq!(totals.working_set_observed, 3);
        assert_eq!(totals.private_commit_observed, 3);
        assert_eq!(totals.sum_unknowns.len(), 2);
        for field in ["working_set_sum_bytes", "private_commit_sum_bytes"] {
            assert!(totals.sum_unknowns.iter().any(|u| u.field == field
                && u.reason == "sum_overflow; reported_bytes_are_a_lower_bound"));
        }
        assert!(totals.enumeration_unknowns.is_empty());
    }

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

    #[cfg(windows)]
    #[test]
    fn leased_process_cadence_preserves_rows_time_and_resets_on_union_change() {
        // #446: Windows charges CPU time in whole clock ticks (15.625 ms by default), so a thread
        // can be charged up to one tick more than it ran. Sleeping two ticks between the own-row
        // reads means this thread runs at most `interval - 2 ticks` and is charged at most
        // `interval - 1 tick`: under one core even on a one-core host (measured here without the
        // sleep: reads 7 ms apart, so one tick alone is 2.2 cores' worth). Other tests sharing the
        // process add only what they really ran, give or take a tick each.
        let two_ticks = std::time::Duration::from_micros(2 * 15_625);
        let mut sampler = Sampler::default();
        let first = sampler.sample_leased(1024, SampleChannels::ALL);
        assert!(sampler.phase_costs().process_snapshot_ms > 0.0);
        std::thread::sleep(two_ticks);
        let second = sampler.sample_leased(1024, SampleChannels::ALL);
        assert_eq!(sampler.phase_costs().process_snapshot_ms, 0.0);
        assert_eq!(sampler.phase_costs().process_walk_ms, 0.0);
        assert_eq!(
            first.processes_observed_at_unix_ms,
            second.processes_observed_at_unix_ms
        );
        assert_eq!(
            serde_json::to_value(&first.processes).unwrap(),
            serde_json::to_value(&second.processes).unwrap()
        );
        let own = sampler.own_process().unwrap();
        assert!(own.private_commit_bytes.is_some());
        assert!(own.rates.cpu_fraction.is_some());
        assert!(own.thread_count.is_none());
        std::thread::sleep(two_ticks);
        sampler.process_sample_at = Some(Instant::now() - std::time::Duration::from_secs(16));
        let refreshed = sampler.sample_leased(1024, SampleChannels::ALL);
        assert!(sampler.phase_costs().process_snapshot_ms > 0.0);
        assert!(refreshed.processes_observed_at_unix_ms >= second.processes_observed_at_unix_ms);
        // The own rate's interval runs from counter read to counter read, not from sample start.
        assert!(sampler.own.as_ref().unwrap().0 > sampler.previous.as_ref().unwrap().0);
        let own = sampler.own_process().unwrap();
        assert!(own.thread_count.is_some());
        assert!(own.rates.cpu_fraction.is_some());
        let gpu_only = SampleChannels {
            system: false,
            processes: false,
            gpu: true,
            disks: false,
            engine: false,
        };
        let gpu_sample = sampler.sample_leased(1024, gpu_only);
        assert!(gpu_sample.processes.is_empty());
        assert!(gpu_sample.processes_observed_at_unix_ms.is_none());
        assert!(sampler.own_process().is_none());
        let restarted = sampler.sample_leased(1024, SampleChannels::ALL);
        assert!(sampler.phase_costs().process_snapshot_ms > 0.0);
        assert!(restarted.sample_interval_seconds.is_none());
        assert!(restarted
            .processes
            .iter()
            .all(|p| p.rates.cpu_fraction.is_none()));
    }

    #[cfg(windows)]
    #[test]
    fn leased_own_row_carries_the_walked_parent_pid_not_a_fabricated_zero() {
        let mut sampler = Sampler::default();
        sampler.sample_leased(1024, SampleChannels::ALL);
        let walked = sampler.own_process().unwrap().parent_pid;
        assert_ne!(walked, 0);
        sampler.sample_leased(1024, SampleChannels::ALL);
        assert_eq!(sampler.phase_costs().process_snapshot_ms, 0.0);
        assert_eq!(sampler.own_process().unwrap().parent_pid, walked);
    }

    #[cfg(windows)]
    #[test]
    fn process_observation_time_is_the_enumerating_sample_start() {
        let sample = Sampler::default().sample(MAX_PROCESS_ROWS);
        assert_eq!(
            sample.processes_observed_at_unix_ms,
            Some(sample.captured_at_unix_ms)
        );
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
        assert_eq!(rates.cpu_fraction, Some(0.125));
        assert_eq!(rates.io_read_bytes_per_s, Some(100.0));
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
            assert!(rates.cpu_fraction.is_none());
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
        assert!(rates.io_read_bytes_per_s.is_none());
        assert!(rates.cpu_fraction.is_none());
        assert!(rates.working_set_delta_bytes.is_none());
        assert_eq!(rates.unknowns.len(), 3);
    }

    #[cfg(windows)]
    #[test]
    fn own_cpu_rate_divides_by_the_counter_read_gap_not_the_sample_start_gap() {
        // #467/#469: seed `own` with a known (read instant, zero CPU) pair 100 s back and push the
        // previous sample start 400 s back. The own rate must be this process's total CPU over the
        // ~100 s read gap; the old start-to-start denominator would give a quarter of that.
        let mut sampler = Sampler::default();
        sampler.sample_leased(1024, SampleChannels::ALL);
        let mut seed = sampler.own_process().unwrap().clone();
        seed.cpu_user_100ns = Some(0);
        seed.cpu_kernel_100ns = Some(0);
        let read_gap = std::time::Duration::from_secs(100);
        let start_gap = std::time::Duration::from_secs(400);
        // `Instant - Duration` panics when the monotonic clock is younger than the gap (a fresh runner).
        let now = Instant::now();
        let (Some(read_at), Some(start_at)) =
            (now.checked_sub(read_gap), now.checked_sub(start_gap))
        else {
            eprintln!("skipped: monotonic clock younger than {start_gap:?}");
            return;
        };
        sampler.own = Some((read_at, seed));
        sampler.previous.as_mut().unwrap().0 = start_at;
        let snapshot = sampler.sample_leased(1024, SampleChannels::ALL);
        let own = sampler.own_process().unwrap();
        let cpu_seconds =
            (own.cpu_user_100ns.unwrap() + own.cpu_kernel_100ns.unwrap()) as f64 / 10_000_000.0;
        let cores = snapshot.logical_processor_count.unwrap() as f64;
        let by_read_gap = cpu_seconds / read_gap.as_secs_f64() / cores;
        let got = own.rates.cpu_fraction.expect("a valid own cpu fraction");
        // The real gap is the seeded 100 s plus the milliseconds the sample took before the read.
        assert!(
            (got - by_read_gap).abs() <= by_read_gap * 0.02,
            "got {got}, expected {by_read_gap} (read gap); start gap would give {}",
            by_read_gap / 4.0
        );
    }

    #[test]
    fn own_cpu_rate_denominator_is_the_read_to_read_gap_over_all_cores() {
        // #469: synthetic instants only, no sleep and no OS read. 40 s of CPU over a 100 s
        // read-to-read gap on 4 logical cores is 0.1; a start-to-start gap of 400 s would give
        // 0.025, and dropping the core count would give 0.4.
        let base = Instant::now();
        let seed = Process {
            pid: 7,
            start_time_windows_100ns: Some("123".into()),
            cpu_user_100ns: Some(0),
            cpu_kernel_100ns: Some(0),
            ..Process::default()
        };
        let own = Process {
            cpu_user_100ns: Some(300_000_000),
            cpu_kernel_100ns: Some(100_000_000),
            ..seed.clone()
        };
        let previous = (base, seed);
        let read_at = base + std::time::Duration::from_secs(100);
        let rates = own_rates(&own, Some(&previous), read_at, 4);
        let got = rates.cpu_fraction.expect("a valid own cpu fraction");
        assert!((got - 0.1).abs() < 1e-12, "got {got}, expected 0.1");
        let no_previous = own_rates(&own, None, read_at, 4);
        assert!(no_previous.cpu_fraction.is_none());
        assert!(no_previous
            .unknowns
            .iter()
            .any(|u| u.reason == "requires_two_samples"));
    }

    #[test]
    fn cached_process_only_ticks_retain_own_row_and_its_observation() {
        let base = Instant::now();
        let channels = SampleChannels {
            engine: false,
            ..SampleChannels::ALL
        };
        let fixture = Snapshot {
            processes: vec![Process {
                pid: std::process::id(),
                parent_pid: 42,
                ..process()
            }],
            processes_observed_at_unix_ms: Some(1000),
            ..unsupported_snapshot()
        };
        let mut sampler = Sampler::default();
        sampler.finish_sample(fixture, 0, channels, true, base, None);
        let own = serde_json::to_value(sampler.own_process().unwrap()).unwrap();
        for seconds in [1, 2] {
            let cached = sampler.finish_sample(
                unsupported_snapshot(),
                0,
                channels,
                false,
                base + std::time::Duration::from_secs(seconds),
                None,
            );
            assert_eq!(
                serde_json::to_value(sampler.own_process().expect("retained own row")).unwrap(),
                own
            );
            assert_eq!(sampler.own.as_ref().unwrap().0, base);
            assert_eq!(cached.processes_observed_at_unix_ms, Some(1000));
            assert!(cached.processes.is_empty()); // Display truncation does not lose the own row.
        }
        // A fresh enumeration without our PID must clear the old observation.
        sampler.finish_sample(
            unsupported_snapshot(),
            0,
            channels,
            true,
            base + std::time::Duration::from_secs(15),
            None,
        );
        assert!(sampler.own_process().is_none());
    }

    #[test]
    fn sampler_own_cpu_uses_and_retains_the_counter_read_instant() {
        // #469: an expensive first walk and a cheaper second read make sample-start and
        // counter-read gaps differ. Synthetic counters/instants, no sleeps or native collection.
        let base = Instant::now();
        let first_read = base + std::time::Duration::from_secs(300);
        let next_start = base + std::time::Duration::from_secs(350);
        let next_read = base + std::time::Duration::from_secs(400);
        let fixture = |cpu_user_100ns| Snapshot {
            logical_processor_count: Some(4),
            processes: vec![Process {
                pid: std::process::id(),
                cpu_user_100ns: Some(cpu_user_100ns),
                ..process()
            }],
            ..unsupported_snapshot()
        };

        for refresh_processes in [false, true] {
            let mut sampler = Sampler::default();
            sampler.finish_sample(
                fixture(0),
                0,
                SampleChannels::ALL,
                true,
                base,
                Some(first_read),
            );
            assert_eq!(sampler.own.as_ref().unwrap().0, first_read);
            assert!(sampler.own_process().unwrap().rates.cpu_fraction.is_none());

            let snapshot = sampler.finish_sample(
                fixture(400_000_000),
                0,
                SampleChannels::ALL,
                refresh_processes,
                next_start,
                Some(next_read),
            );
            // 40 CPU seconds / (100 read-to-read seconds * 4 logical cores).
            let got = sampler.own_process().unwrap().rates.cpu_fraction.unwrap();
            assert!((got - 0.1).abs() < 1e-12, "got {got}, expected 0.1");
            assert_eq!(sampler.own.as_ref().unwrap().0, next_read);
            assert_eq!(snapshot.sample_interval_seconds, Some(350.0));
            assert!(snapshot.processes.is_empty());
        }
    }

    #[test]
    fn own_read_instant_is_the_counter_read_and_falls_back_to_the_sample_start() {
        // #469: the own rate and the stored previous read take the counter-read instant, never the
        // call site's `now`. Synthetic instants (the read is 3 ms after the sample start), no OS read.
        let sample_started = Instant::now();
        let counter_read = sample_started + std::time::Duration::from_millis(3);
        assert_eq!(
            own_read_instant(Some(counter_read), sample_started),
            counter_read
        );
        assert_eq!(own_read_instant(None, sample_started), sample_started);
    }

    #[test]
    fn cpu_denominator_is_machine_capacity_and_impossible_rates_are_unknown() {
        let previous = process();
        let current = Process {
            cpu_user_100ns: Some(20_000_000),
            ..previous.clone()
        };
        assert_eq!(
            derive_rates(&current, &previous, 1.0, 20).cpu_fraction,
            Some(0.1)
        );
        assert_eq!(
            derive_rates(&current, &previous, 1.0, 2).cpu_fraction,
            Some(1.0)
        );
        for cores in [0, 1] {
            let rates = derive_rates(&current, &previous, 1.0, cores);
            assert!(rates.cpu_fraction.is_none());
            assert!(rates.unknowns.iter().any(|u| u.field == "cpu_fraction"));
        }
    }
}
