//! LW-121: low-space and fast-growth alerts as a pure function of already-observed facts.
//!
//! Nothing here reads a disk, a clock or a store: the service passes the volume space it already
//! observes for `volumes.list` and the completed-scan history the catalogue keeps, plus `now`.
//! Unknown, missing and partial inputs become statuses that explain why no alert was raised; an
//! unobserved number is never treated as zero (AGENTS.md invariant 4). Alerts are advice only.

use crate::events::timestamp_ns;
use loomward_protocol::*;

const GIB: u64 = 1 << 30;
const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// One volume that holds an active root grant, as `volumes.list` observes it.
#[derive(Debug, Clone)]
pub struct VolumeInput {
    /// Wire identifier of the volume.
    pub volume_id: VolumeId,
    /// Display name, cut to the wire bound by [`evaluate`].
    pub label: String,
    /// Whether the volume is currently online.
    pub online: bool,
    /// Observed capacity; `None` is unknown.
    pub capacity_bytes: Option<u64>,
    /// Observed free space; `None` is unknown.
    pub free_bytes: Option<u64>,
}

/// The size of a root at the end of one completed scan run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// When the run finished, in nanoseconds since the Unix epoch.
    pub finished_at_ns: i64,
    /// The root's logical bytes at that time.
    pub logical_bytes: u64,
    /// Whether that run covered the whole root with no skipped or failed entries.
    pub complete: bool,
}

/// One active root and its completed scans, oldest first.
#[derive(Debug, Clone)]
pub struct RootInput {
    /// Wire identifier of the root.
    pub root_id: RootId,
    /// Display label, cut to the wire bound by [`evaluate`].
    pub label: String,
    /// Completed runs that recorded a size; runs without one are not samples and are left out.
    pub samples: Vec<Sample>,
}

/// The documented defaults (`contracts/v3/semantics.md`): notifications off, warn below the larger
/// of 10% and 10 GiB free, and alert on 5 GiB of net growth within 7 days.
pub fn default_settings() -> AlertSettings {
    AlertSettings {
        notifications_enabled: false,
        low_space_free_percent: Int::try_from(10).expect("default percent"),
        low_space_free_bytes: Bytes(10 * GIB),
        growth_window_days: Int::try_from(7).expect("default window"),
        growth_bytes: Bytes(5 * GIB),
    }
}

fn note(v: &str) -> Text<256> {
    Text::truncated(v)
}

fn item(kind: AlertKind, status: AlertStatus, label: &str) -> AlertItem {
    AlertItem {
        kind,
        status,
        label: Text::truncated(label),
        volume_id: None,
        root_id: None,
        measured_bytes: None,
        threshold_bytes: None,
        capacity_bytes: None,
        window_start: None,
        window_end: None,
        coverage: CoverageState::Unknown,
        note: note(""),
    }
}

fn low_space(s: &AlertSettings, v: &VolumeInput) -> AlertItem {
    let mut i = item(AlertKind::LowSpace, AlertStatus::Unknown, &v.label);
    i.volume_id = Some(v.volume_id.clone());
    i.capacity_bytes = v.capacity_bytes.map(Bytes);
    i.measured_bytes = v.free_bytes.map(Bytes);
    let (Some(capacity), Some(free), true) = (v.capacity_bytes, v.free_bytes, v.online) else {
        i.measured_bytes = None;
        i.note = note(if v.online {
            "Capacity or free space was not observed for this volume; no alert is raised and none is implied."
        } else {
            "The volume is offline; its free space is unknown, so no alert is raised."
        });
        return i;
    };
    let by_percent = u64::try_from(
        u128::from(capacity) * u128::from(s.low_space_free_percent.get() as u64) / 100,
    )
    .unwrap_or(u64::MAX);
    let threshold = by_percent.max(s.low_space_free_bytes.get());
    i.threshold_bytes = Some(Bytes(threshold));
    i.coverage = CoverageState::Complete;
    if free < threshold {
        i.status = AlertStatus::Alert;
        i.note = note("Free space is below the threshold (the larger of the percentage of capacity and the byte floor).");
    } else {
        i.status = AlertStatus::Ok;
        i.note = note("Free space is at or above the threshold.");
    }
    i
}

fn fast_growth(s: &AlertSettings, r: &RootInput, now_ns: i64) -> AlertItem {
    let mut i = item(
        AlertKind::FastGrowth,
        AlertStatus::InsufficientHistory,
        &r.label,
    );
    i.root_id = Some(r.root_id.clone());
    i.threshold_bytes = Some(s.growth_bytes);
    let from = now_ns.saturating_sub(s.growth_window_days.get().saturating_mul(DAY_NS));
    let inside: Vec<&Sample> = r
        .samples
        .iter()
        .filter(|p| p.finished_at_ns >= from)
        .collect();
    let (Some(first), Some(newest)) = (inside.first(), inside.last()) else {
        i.note =
            note("No completed scan of this root inside the growth window; no growth is claimed.");
        return i;
    };
    i.window_start = Some(timestamp_ns(first.finished_at_ns));
    i.window_end = Some(timestamp_ns(newest.finished_at_ns));
    if !newest.complete {
        i.status = AlertStatus::IncompleteCoverage;
        i.coverage = CoverageState::Partial;
        i.note = note("The latest scan did not cover the whole root, so its size is not comparable; no growth is claimed.");
        return i;
    }
    let complete: Vec<&&Sample> = inside.iter().filter(|p| p.complete).collect();
    if complete.len() < 2 {
        let partial = inside.iter().any(|p| !p.complete);
        i.status = if partial {
            AlertStatus::IncompleteCoverage
        } else {
            AlertStatus::InsufficientHistory
        };
        i.coverage = if partial {
            CoverageState::Partial
        } else {
            CoverageState::Unknown
        };
        i.note = note(if partial {
            "Only one complete scan is inside the window and the others were partial; no growth is claimed."
        } else {
            "Fewer than two complete scans are inside the window; no growth is claimed."
        });
        return i;
    }
    let (oldest, newest) = (complete[0], complete[complete.len() - 1]);
    i.window_start = Some(timestamp_ns(oldest.finished_at_ns));
    i.window_end = Some(timestamp_ns(newest.finished_at_ns));
    let growth = newest.logical_bytes.saturating_sub(oldest.logical_bytes);
    i.measured_bytes = Some(Bytes(growth));
    i.coverage = CoverageState::Complete;
    if growth > 0 && growth >= s.growth_bytes.get() {
        i.status = AlertStatus::Alert;
        i.note = note("Net logical growth between the oldest and newest complete scans in the window reaches the threshold.");
    } else {
        i.status = AlertStatus::Ok;
        i.note = note("Net logical growth between the oldest and newest complete scans in the window is below the threshold.");
    }
    i
}

/// Evaluates every volume and root. Volumes first, each list in the order given.
pub fn evaluate(
    settings: &AlertSettings,
    volumes: &[VolumeInput],
    roots: &[RootInput],
    now_ns: i64,
) -> AlertReport {
    let mut items: Vec<AlertItem> = volumes.iter().map(|v| low_space(settings, v)).collect();
    items.extend(roots.iter().map(|r| fast_growth(settings, r, now_ns)));
    items.truncate(128);
    let active = items
        .iter()
        .filter(|i| i.status == AlertStatus::Alert)
        .count();
    AlertReport {
        evaluated_at: timestamp_ns(now_ns),
        settings: settings.clone(),
        defaults: default_settings(),
        active_count: Count::saturating(active as u64),
        items: BoundedVec::new(items).expect("at most 128 alert items"),
        notifications_note: note(if settings.notifications_enabled {
            "Notifications are enabled, but no operating-system notification channel is wired yet; alerts show in the app only."
        } else {
            "Notifications are off. Alerts show in the app only; no operating-system notification channel is wired yet."
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 100 * DAY_NS;

    fn vol(id: &str, online: bool, capacity: Option<u64>, free: Option<u64>) -> VolumeInput {
        VolumeInput {
            volume_id: VolumeId::new(id).unwrap(),
            label: id.into(),
            online,
            capacity_bytes: capacity,
            free_bytes: free,
        }
    }

    fn root(samples: &[(i64, u64, bool)]) -> RootInput {
        RootInput {
            root_id: RootId::new("rt_1").unwrap(),
            label: "Synthetic root".into(),
            samples: samples
                .iter()
                .map(|&(days_ago, bytes, complete)| Sample {
                    finished_at_ns: NOW - days_ago * DAY_NS,
                    logical_bytes: bytes,
                    complete,
                })
                .collect(),
        }
    }

    fn status(volumes: &[VolumeInput], roots: &[RootInput]) -> Vec<AlertStatus> {
        evaluate(&default_settings(), volumes, roots, NOW)
            .items
            .iter()
            .map(|i| i.status)
            .collect()
    }

    #[test]
    fn low_space_uses_the_larger_of_percent_and_bytes() {
        // 1 TiB: 10% (~102 GiB) beats the 10 GiB floor; 20 GiB: the floor beats 10% (2 GiB).
        let tib = 1024 * GIB;
        let big = |free| vol("vo_1", true, Some(tib), Some(free));
        assert_eq!(status(&[big(101 * GIB)], &[]), [AlertStatus::Alert]);
        assert_eq!(status(&[big(103 * GIB)], &[]), [AlertStatus::Ok]);
        let small = |free| vol("vo_2", true, Some(20 * GIB), Some(free));
        assert_eq!(status(&[small(9 * GIB)], &[]), [AlertStatus::Alert]);
        assert_eq!(status(&[small(11 * GIB)], &[]), [AlertStatus::Ok]);
        let item = &evaluate(&default_settings(), &[small(9 * GIB)], &[], NOW).items[0];
        assert_eq!(item.threshold_bytes, Some(Bytes(10 * GIB)));
        assert_eq!(item.measured_bytes, Some(Bytes(9 * GIB)));
        assert_eq!(item.coverage, CoverageState::Complete);
    }

    #[test]
    fn unknown_or_offline_space_is_suppressed_never_zero() {
        let report = evaluate(
            &default_settings(),
            &[
                vol("vo_1", true, None, None),
                vol("vo_2", true, Some(100 * GIB), None),
                vol("vo_3", false, Some(100 * GIB), Some(0)),
            ],
            &[],
            NOW,
        );
        assert!(report
            .items
            .iter()
            .all(|i| i.status == AlertStatus::Unknown));
        assert!(report
            .items
            .iter()
            .all(|i| i.measured_bytes.is_none() && i.threshold_bytes.is_none()));
        assert!(report
            .items
            .iter()
            .all(|i| i.coverage == CoverageState::Unknown));
        assert_eq!(report.active_count.get(), 0);
    }

    #[test]
    fn growth_alerts_on_complete_scans_inside_the_window() {
        let gib = |n: u64| n * GIB;
        // +6 GiB over 5 days is above the 5 GiB default; a scan 20 days old is outside the window.
        let r = root(&[(20, gib(1), true), (5, gib(10), true), (1, gib(16), true)]);
        let report = evaluate(&default_settings(), &[], &[r], NOW);
        let item = &report.items[0];
        assert_eq!(item.status, AlertStatus::Alert);
        assert_eq!(item.measured_bytes, Some(Bytes(gib(6))));
        assert_eq!(item.coverage, CoverageState::Complete);
        assert!(item.window_start.is_some() && item.window_end.is_some());
        assert_eq!(report.active_count.get(), 1);
        // Just under the threshold is ok; shrinking is a measured zero, still ok.
        assert_eq!(
            status(&[], &[root(&[(5, gib(10), true), (1, gib(14), true)])]),
            [AlertStatus::Ok]
        );
        let shrink = evaluate(
            &default_settings(),
            &[],
            &[root(&[(5, gib(10), true), (1, gib(2), true)])],
            NOW,
        );
        assert_eq!(shrink.items[0].status, AlertStatus::Ok);
        assert_eq!(shrink.items[0].measured_bytes, Some(Bytes(0)));
    }

    #[test]
    fn partial_scans_qualify_instead_of_reading_as_growth_or_zero() {
        let gib = |n: u64| n * GIB;
        // A huge jump to a partial latest scan is not growth: coverage is the story.
        let latest_partial = evaluate(
            &default_settings(),
            &[],
            &[root(&[(5, gib(1), true), (1, gib(50), false)])],
            NOW,
        );
        let i = &latest_partial.items[0];
        assert_eq!(
            (i.status, i.coverage),
            (AlertStatus::IncompleteCoverage, CoverageState::Partial)
        );
        assert_eq!(i.measured_bytes, None);
        assert_eq!(latest_partial.active_count.get(), 0);
        // One complete scan after partial ones: still not comparable.
        assert_eq!(
            status(&[], &[root(&[(5, gib(1), false), (1, gib(50), true)])]),
            [AlertStatus::IncompleteCoverage]
        );
        // A partial scan between two complete ones does not hide real growth.
        assert_eq!(
            status(
                &[],
                &[root(&[
                    (5, gib(1), true),
                    (3, gib(2), false),
                    (1, gib(9), true)
                ])]
            ),
            [AlertStatus::Alert]
        );
    }

    #[test]
    fn missing_history_is_insufficient_not_zero() {
        assert_eq!(
            status(&[], &[root(&[])]),
            [AlertStatus::InsufficientHistory]
        );
        assert_eq!(
            status(&[], &[root(&[(1, 99 * GIB, true)])]),
            [AlertStatus::InsufficientHistory]
        );
        // Everything is older than the 7-day window.
        assert_eq!(
            status(&[], &[root(&[(30, GIB, true), (20, 90 * GIB, true)])]),
            [AlertStatus::InsufficientHistory]
        );
        let none = evaluate(&default_settings(), &[], &[root(&[])], NOW);
        assert_eq!(none.items[0].measured_bytes, None);
        assert_eq!(none.items[0].threshold_bytes, Some(Bytes(5 * GIB)));
    }

    #[test]
    fn thresholds_come_from_the_settings() {
        let mut s = default_settings();
        s.growth_bytes = Bytes(GIB);
        s.growth_window_days = Int::try_from(30).unwrap();
        s.low_space_free_percent = Int::try_from(50).unwrap();
        s.low_space_free_bytes = Bytes(0);
        s.notifications_enabled = true;
        let report = evaluate(
            &s,
            &[vol("vo_1", true, Some(100 * GIB), Some(40 * GIB))],
            &[root(&[(20, GIB, true), (1, 3 * GIB, true)])],
            NOW,
        );
        assert_eq!(report.active_count.get(), 2);
        assert_eq!(report.defaults, default_settings());
        assert!(report.notifications_note.contains("enabled"));
    }

    #[test]
    fn the_report_is_valid_on_the_wire() {
        let report = evaluate(
            &default_settings(),
            &[vol("vo_1", true, None, None)],
            &[root(&[])],
            NOW,
        );
        let wire = serde_json::to_value(&report).unwrap();
        let back: AlertReport = decode_exact(wire).unwrap();
        assert_eq!(back, report);
    }
}
