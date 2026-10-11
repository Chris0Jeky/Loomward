//! LW-051: bounded observations and conservative candidates, never diagnoses or authority.
use super::mapping;
use loomward_protocol::*;
use loomward_telemetry::{Memory, Observation, Snapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const HISTORY_MS: u64 = 600_000;
const POINTS: usize = 8;
const MIN_SAMPLES: usize = 4;

#[derive(Debug, Default)]
pub(super) struct History {
    series: BTreeMap<(String, TrendMetric), Arc<TrendSeries>>,
    process_at: Option<u64>,
    process_notes: Vec<Text<256>>,
    channels: Option<Vec<TelemetryChannel>>,
}

fn note(v: &str) -> Text<256> {
    Text::truncated(v)
}

fn number(v: Option<f64>, metric: TrendMetric) -> Option<TrendValue> {
    v.filter(|v| metric.unit() != TrendUnit::Fraction || *v <= 1.0)
        .and_then(|v| Rate::new(v).ok())
        .map(TrendValue::Number)
}

fn bytes(v: Option<u64>) -> Option<TrendValue> {
    v.map(|v| TrendValue::Bytes(v.into()))
}

impl History {
    fn enabled(&self, channel: TelemetryChannel) -> bool {
        self.channels
            .as_ref()
            .is_none_or(|channels| channels.contains(&channel))
    }

    pub(super) fn retain_channels(&mut self, channels: &[TelemetryChannel]) {
        if self.channels.as_ref().is_some_and(|old| {
            old.len() == channels.len() && old.iter().all(|c| channels.contains(c))
        }) {
            return;
        }
        self.series
            .retain(|(_, metric), _| channels.contains(&metric.channel()));
        if !channels.contains(&TelemetryChannel::Processes) {
            self.process_at = None;
            self.process_notes.clear();
        }
        self.channels = Some(channels.to_vec());
    }

    fn record(
        &mut self,
        key: &str,
        metric: TrendMetric,
        value: Option<TrendValue>,
        at_ms: u64,
        at: &Timestamp,
        cadence_ms: u64,
    ) {
        let s = self
            .series
            .entry((key.to_owned(), metric))
            .or_insert_with(|| {
                Arc::new(TrendSeries {
                    key: Text::truncated(key),
                    metric,
                    unit: metric.unit(),
                    max_gap_ms: Count::saturating(cadence_ms * 2),
                    points: BoundedVec::default(),
                    unknowns: BoundedVec::default(),
                })
            });
        // Process histories are unchanged between enumerations; snapshots share immutable versions.
        let s = Arc::make_mut(s);
        let cadence_changed = s.max_gap_ms.get() != cadence_ms * 2;
        s.max_gap_ms = Count::saturating(cadence_ms * 2);
        let mut points = std::mem::take(&mut s.points).into_vec();
        if cadence_changed {
            points.clear();
        }
        points.retain(|p| at_ms.saturating_sub(p.offset_ms.get()) <= HISTORY_MS);
        if points.len() == POINTS {
            points.remove(0);
        }
        points.push(TrendPoint {
            offset_ms: Count::saturating(at_ms),
            observed_at: at.clone(),
            value,
        });
        s.points = points.try_into().expect("bounded trend points");
        let mut unknowns = vec![];
        if cadence_changed {
            unknowns.push(note(
                "Cadence changed; earlier observation window discarded.",
            ));
        }
        if s.points.iter().any(|p| p.value.is_none()) {
            unknowns.push(note(
                "Missing, denied or unsupported samples remain gaps, not zero.",
            ));
        }
        if s.points
            .windows(2)
            .any(|p| p[1].offset_ms.get().saturating_sub(p[0].offset_ms.get()) > s.max_gap_ms.get())
        {
            unknowns.push(note(
                "Sampling gap exceeds two requested intervals; no candidate crosses it.",
            ));
        }
        if metric.channel() == TelemetryChannel::Processes {
            unknowns.push(note(
                "Shared resident memory is not measured uniquely; process working sets overlap.",
            ));
        }
        s.unknowns = unknowns.try_into().expect("bounded trend unknowns");
    }

    pub(super) fn push(&mut self, raw: &Snapshot, at_ms: u64, interval_ms: u64) {
        let Ok(at) = mapping::observed_at(raw) else {
            return;
        };
        if raw.sampled_channels.system && self.enabled(TelemetryChannel::System) {
            let empty = Memory::default();
            let m = match &raw.memory {
                Observation::Observed { value } => value,
                _ => &empty,
            };
            for (metric, v) in [
                (TrendMetric::TotalBytes, m.total_bytes),
                (TrendMetric::AvailableBytes, m.available_bytes),
                (TrendMetric::CommitBytes, m.commit_bytes),
                (TrendMetric::CommitLimitBytes, m.commit_limit_bytes),
                (TrendMetric::CommitPeakBytes, m.commit_peak_bytes),
                (TrendMetric::SystemCacheBytes, m.system_cache_bytes),
                (TrendMetric::KernelPagedBytes, m.kernel_paged_bytes),
                (TrendMetric::KernelNonpagedBytes, m.kernel_nonpaged_bytes),
                (TrendMetric::PageSizeBytes, m.page_size_bytes),
                (TrendMetric::StandbyBytes, m.standby_bytes),
                (TrendMetric::ModifiedBytes, m.modified_bytes),
                (TrendMetric::FreeBytes, m.free_bytes),
            ] {
                self.record("system", metric, bytes(v), at_ms, &at, interval_ms);
            }
            self.record(
                "system",
                TrendMetric::CpuBusyFraction,
                number(raw.system_cpu_busy_fraction, TrendMetric::CpuBusyFraction),
                at_ms,
                &at,
                interval_ms,
            );
        }
        if raw.sampled_channels.disks && self.enabled(TelemetryChannel::Disks) {
            let disks = match &raw.disk_io {
                Observation::Observed { value } => value.as_slice(),
                _ => &[],
            };
            let mut keys = BTreeSet::new();
            for (i, d) in disks.iter().take(16).enumerate() {
                let key = format!("disk_{i}:{}", d.disk_label);
                keys.insert(key.clone());
                let unique = disks
                    .iter()
                    .filter(|other| other.disk_label == d.disk_label)
                    .count()
                    == 1;
                for (metric, v) in [
                    (TrendMetric::DiskBusyFraction, d.busy_fraction),
                    (TrendMetric::DiskQueueLength, d.queue_length),
                ] {
                    self.record(
                        &key,
                        metric,
                        number(v.filter(|_| unique), metric),
                        at_ms,
                        &at,
                        interval_ms,
                    );
                }
            }
            if matches!(&raw.disk_io, Observation::Observed { .. }) {
                self.series.retain(|(key, metric), _| {
                    metric.channel() != TelemetryChannel::Disks || keys.contains(key)
                });
            }
            let missing: Vec<_> = self
                .series
                .keys()
                .filter(|(key, metric)| {
                    metric.channel() == TelemetryChannel::Disks && !keys.contains(key)
                })
                .cloned()
                .collect();
            for (key, metric) in missing {
                self.record(&key, metric, None, at_ms, &at, interval_ms);
            }
        }
        if raw.sampled_channels.processes
            && self.enabled(TelemetryChannel::Processes)
            && raw.processes_observed_at_unix_ms != self.process_at
        {
            self.process_at = raw.processes_observed_at_unix_ms;
            self.process_notes.clear();
            let process_at = match mapping::processes_observed_at(raw) {
                Ok(at) => at,
                Err(_) => {
                    self.process_notes.push(note("Process provider observation is missing; retained instances now have a gap."));
                    let keys: Vec<_> = self
                        .series
                        .keys()
                        .filter(|(_, metric)| metric.channel() == TelemetryChannel::Processes)
                        .cloned()
                        .collect();
                    for (key, metric) in keys {
                        self.record(&key, metric, None, at_ms, &at, interval_ms.max(15_000));
                    }
                    return;
                }
            };
            let mut keys = BTreeSet::new();
            for p in raw
                .processes
                .iter()
                .take(loomward_telemetry::MAX_PROCESS_ROWS)
            {
                let key = mapping::process_ref(p, 0).as_str().to_owned();
                keys.insert(key.clone());
                let identity = p
                    .start_time_windows_100ns
                    .as_deref()
                    .filter(|v| v.len() <= 20 && v.bytes().all(|c| c.is_ascii_digit()))
                    .and_then(|v| v.parse::<u64>().ok())
                    .is_some();
                let denied = p.access_denied();
                if (!identity || denied) && self.process_notes.is_empty() {
                    self.process_notes.push(note("Denied handles or missing creation time prevent exact-instance growth comparison."));
                }
                let valid =
                    identity && !denied && raw.process_totals.enumeration_unknowns.is_empty();
                for (metric, v) in [
                    (TrendMetric::PrivateCommitBytes, p.private_commit_bytes),
                    (
                        TrendMetric::PrivateWorkingSetBytes,
                        p.private_working_set_bytes,
                    ),
                    (TrendMetric::WorkingSetBytes, p.working_set_bytes),
                    (TrendMetric::GpuDedicatedBytes, None),
                ] {
                    self.record(
                        &key,
                        metric,
                        bytes(v.filter(|_| valid)),
                        at_ms,
                        &process_at,
                        interval_ms.max(15_000),
                    );
                }
                for (metric, v) in [
                    (TrendMetric::CpuFraction, p.rates.cpu_fraction),
                    (TrendMetric::IoReadBytesPerS, p.rates.io_read_bytes_per_s),
                    (TrendMetric::IoWriteBytesPerS, p.rates.io_write_bytes_per_s),
                ] {
                    self.record(
                        &key,
                        metric,
                        number(v.filter(|_| valid), metric),
                        at_ms,
                        &process_at,
                        interval_ms.max(15_000),
                    );
                }
            }
            let before = self.series.len();
            self.series.retain(|(key, metric), _| {
                metric.channel() != TelemetryChannel::Processes || keys.contains(key)
            });
            if before > self.series.len() {
                self.process_notes.push(note("Process disappearance, restart or PID reuse ended earlier series; instances are never joined."));
            }
            if raw.display_truncated || raw.process_totals.enumeration_truncated {
                self.process_notes.push(note("Process coverage is truncated; unobserved processes and shared memory remain unknown."));
            }
        }
        // 1,024 process rows x 7 metrics + 16 disks x 2 + 13 machine series.
        debug_assert!(self.series.len() <= 7213);
    }

    pub(super) fn report(&self, channels: &[TelemetryChannel]) -> TelemetryTrends {
        let series: Vec<_> = self
            .series
            .values()
            .filter(|s| channels.contains(&s.metric.channel()))
            .cloned()
            .collect();
        let mut candidates = vec![];
        for (i, s) in series.iter().enumerate() {
            if let Some(c) = candidate(s, i, &series, &self.process_notes) {
                candidates.push(c);
            }
        }
        // The cap cuts the tail, so machine candidates go first, then disks, then the many processes.
        candidates.sort_by_key(
            |c| match series[c.series_index.get() as usize].metric.channel() {
                TelemetryChannel::System => 0,
                TelemetryChannel::Disks => 1,
                _ => 2,
            },
        );
        let candidates_truncated = candidates.len() > 64;
        candidates.truncate(64);
        TelemetryTrends {
            series: series.try_into().expect("bounded trend series"),
            candidates: candidates.try_into().expect("bounded candidates"),
            candidates_truncated,
            unknowns: if channels.contains(&TelemetryChannel::Processes) {
                self.process_notes.clone()
            } else {
                vec![]
            }
            .try_into()
            .expect("bounded coverage notes"),
        }
    }
}

/// SSE keeps the existing summary-row scope; snapshots retain all leased series.
pub(super) fn event_projection(
    mut report: TelemetryTrends,
    summary: Option<&ProcessSummary>,
) -> TelemetryTrends {
    let keys: BTreeSet<_> = summary
        .into_iter()
        .flat_map(|s| &s.top)
        .map(|r| r.process_ref.as_str())
        .collect();
    let mut indices = BTreeMap::new();
    let mut series = vec![];
    let old = std::mem::take(&mut report.series).into_vec();
    let before = old.len();
    for (i, s) in old.into_iter().enumerate() {
        if s.metric.channel() != TelemetryChannel::Processes || keys.contains(s.key.as_str()) {
            indices.insert(i, series.len());
            series.push(s);
        }
    }
    if series.len() < before {
        let mut notes = std::mem::take(&mut report.unknowns).into_vec();
        notes.insert(0, note("Event process trends cover summary rows only; full leased history is available in telemetry.snapshot."));
        notes.truncate(8);
        report.unknowns = notes.try_into().expect("bounded coverage notes");
    }
    report.series = series.try_into().expect("bounded event series");
    report.candidates = std::mem::take(&mut report.candidates)
        .into_vec()
        .into_iter()
        .filter_map(|mut c| {
            let new = indices.get(&(c.series_index.get() as usize))?;
            c.series_index = Count::saturating(*new as u64);
            Some(c)
        })
        .collect::<Vec<_>>()
        .try_into()
        .expect("bounded event candidates");
    report
}

fn value(v: &TrendValue) -> f64 {
    match v {
        TrendValue::Bytes(b) => b.get() as f64,
        TrendValue::Number(n) => n.get(),
    }
}

fn display(v: &TrendValue) -> String {
    match v {
        TrendValue::Bytes(b) => b.get().to_string(),
        TrendValue::Number(n) => n.get().to_string(),
    }
}

fn difference(a: &TrendValue, b: &TrendValue) -> f64 {
    match (a, b) {
        (TrendValue::Bytes(a), TrendValue::Bytes(b)) => {
            (i128::from(b.get()) - i128::from(a.get())) as f64
        }
        _ => value(b) - value(a),
    }
}

/// The newest run of known points with in-bounds spacing. A missing value, a repeated or backward
/// timestamp or a gap wider than `max_gap_ms` ends the run behind it, so it never lets one bad
/// point hide a later window; no candidate ever crosses one.
fn segment(s: &TrendSeries) -> Option<&[TrendPoint]> {
    let points: &[TrendPoint] = &s.points;
    let mut start = points.len();
    while start > 0 && points[start - 1].value.is_some() {
        if start < points.len() {
            let dt = points[start]
                .offset_ms
                .get()
                .saturating_sub(points[start - 1].offset_ms.get());
            if dt == 0 || dt > s.max_gap_ms.get() {
                break;
            }
        }
        start -= 1;
    }
    (points.len() - start >= MIN_SAMPLES).then(|| &points[start..])
}

fn values<'a>(points: impl IntoIterator<Item = &'a TrendPoint>) -> Vec<&'a TrendValue> {
    points
        .into_iter()
        .filter_map(|p| p.value.as_ref())
        .collect()
}

fn candidate(
    s: &TrendSeries,
    index: usize,
    series: &[Arc<TrendSeries>],
    notes: &[Text<256>],
) -> Option<TrendCandidate> {
    let points = segment(s)?;
    let v = values(points);
    let first = points.first()?;
    let last = points.last()?;
    let ms = last.offset_ms.get().checked_sub(first.offset_ms.get())?;
    let start = v.first()?;
    let end = v.last()?;
    let delta = difference(start, end);
    let mut facts = vec![];
    let mut missing = vec![];
    let kind = match s.metric {
        TrendMetric::PrivateCommitBytes | TrendMetric::PrivateWorkingSetBytes => {
            if ms < 30_000
                || delta < (1 << 20) as f64
                || v.windows(2).any(|p| difference(p[0], p[1]) < 0.0)
                || v.windows(2)
                    .filter(|p| difference(p[0], p[1]) > 0.0)
                    .count()
                    < 3
            {
                return None;
            }
            facts.push(note("At least four exact-instance samples span 30 s or more, with no decrease and at least three increases totalling at least 1 MiB."));
            TrendCandidateKind::Growth
        }
        TrendMetric::CpuBusyFraction => {
            if ms < 3000 || !v.iter().all(|v| value(v) >= 0.9) {
                return None;
            }
            facts.push(note("Every machine CPU sample in this window was at least 0.90 of logical CPU capacity."));
            TrendCandidateKind::CpuSaturation
        }
        TrendMetric::DiskQueueLength => {
            if ms < 3000 || !v.iter().all(|v| value(v) >= 2.0) {
                return None;
            }
            facts.push(note("Every observed average disk queue length in this window was at least 2; busy fraction is a separate measure."));
            TrendCandidateKind::DiskQueue
        }
        TrendMetric::AvailableBytes => {
            // A companion series counts only if it is known at exactly this run's timestamps.
            let aligned = |metric| -> Option<Vec<&TrendValue>> {
                let other = series
                    .iter()
                    .find(|o| o.key == s.key && o.metric == metric)?;
                let same: Vec<_> = other
                    .points
                    .iter()
                    .filter(|o| points.iter().any(|p| p.offset_ms == o.offset_ms))
                    .collect();
                let known = values(same.iter().copied());
                (same.len() == points.len() && known.len() == same.len()).then_some(known)
            };
            let totals = aligned(TrendMetric::TotalBytes)?;
            if ms < 3000
                || !v
                    .iter()
                    .zip(totals)
                    .all(|(a, t)| value(t) > 0.0 && value(a) / value(t) <= 0.1)
            {
                return None;
            }
            let movement = |metric, rising| {
                aligned(metric).is_some_and(|v| {
                    let delta = difference(v[0], v[v.len() - 1]);
                    if rising {
                        delta > 0.0
                    } else {
                        delta < 0.0
                    }
                })
            };
            if !movement(TrendMetric::StandbyBytes, false)
                && !movement(TrendMetric::ModifiedBytes, true)
            {
                return None;
            }
            for metric in [TrendMetric::StandbyBytes, TrendMetric::ModifiedBytes] {
                if let Some(v) = aligned(metric) {
                    facts.push(note(&format!(
                        "{metric:?}: {} to {} bytes over the same window.",
                        display(v[0]),
                        display(v[v.len() - 1])
                    )));
                } else {
                    missing.push(note(&format!(
                        "{metric:?} samples are missing or gapped in this window."
                    )));
                }
            }
            facts.push(note("Available memory stayed at or below 10% of total while standby declined or modified bytes increased."));
            TrendCandidateKind::MemoryPressure
        }
        _ => return None,
    };
    facts.insert(
        0,
        note(&format!(
            "{} {:?}: {} to {} {:?}; {} samples over {} ms.",
            s.key.as_str(),
            s.metric,
            display(start),
            display(end),
            s.unit,
            v.len(),
            ms
        )),
    );
    let mut unknowns = vec![note("Shared memory, denied handles and process restarts/PID reuse prevent complete workload attribution. These thresholds name candidates, not diagnoses or probabilities.")];
    unknowns.extend(missing);
    unknowns.extend(s.unknowns.iter().cloned());
    if s.metric.channel() == TelemetryChannel::Processes {
        unknowns.push(note("Exact creation time separates restarts and PID reuse; observations outside this window and denied handles remain unknown."));
        unknowns.extend(notes.iter().cloned());
    } else {
        unknowns.push(note("Missing or gapped samples prevent a candidate window; shorter unseen bursts and provider coverage remain unknown."));
        if s.metric.channel() == TelemetryChannel::Disks {
            unknowns.push(note("Disk labels and enumeration order are observation keys, not durable device identity. Queue thresholds are not hardware capacity limits."));
        }
    }
    unknowns.truncate(8);
    Some(TrendCandidate {
        kind,
        series_index: Count::saturating(index as u64),
        unit: s.unit,
        start_at: first.observed_at.clone(),
        end_at: last.observed_at.clone(),
        window_ms: Count::saturating(ms),
        sample_count: Count::saturating(v.len() as u64),
        start_value: (*start).clone(),
        end_value: (*end).clone(),
        slope_per_s: Text::truncated(&format!("{:.6}", delta * 1000.0 / ms as f64)),
        facts: facts.try_into().expect("bounded facts"),
        unknowns: unknowns.try_into().expect("bounded unknowns"),
    })
}

#[cfg(test)]
mod tests;
