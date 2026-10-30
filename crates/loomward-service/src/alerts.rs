//! LW-121: `alerts.list` and `alerts.configure`. Alerts are advice evaluated on demand from facts the
//! service already holds: the volume space observation `volumes.list` reports, and the size each
//! completed scan run recorded for its root. No scheduler, no network, no file or process effect.
//! The only write is the owner's threshold record in `state.db` (`meta` key `alert_settings`).
use crate::placement::Input;
use crate::{db, fail, now_ns, sql_error, Inner, Out, ReadWork};
use loomward_engine::alerts::{self, RootInput, Sample, VolumeInput};
use loomward_protocol::*;
use rusqlite::Connection;
use serde_json::Value;
use std::time::{Duration, Instant};

type Handled = Result<Out, ErrorBody>;

/// Newest completed runs read per root; far more than any window of weekly or daily scans needs.
const MAX_SAMPLES: i64 = 64;

fn settings(conn: &Connection) -> Result<AlertSettings, ErrorBody> {
    // A record that no longer parses (a newer build wrote it) falls back to the defaults.
    Ok(db::alert_settings(conn)
        .map_err(sql_error)?
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_else(alerts::default_settings))
}

/// Completed runs of `root` that recorded a size, oldest first. A run without the counters
/// (an older row, or a catalogue rebuilt since) is not a sample: unknown, never zero.
fn samples(conn: &Connection, root: i64) -> Result<Vec<Sample>, ErrorBody> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT finished_at_ns,counters_json FROM main.scan_run
             WHERE root_id=?1 AND state='completed' AND finished_at_ns IS NOT NULL
             ORDER BY finished_at_ns DESC,id DESC LIMIT ?2",
        )
        .map_err(sql_error)?;
    let rows = stmt
        .query_map(rusqlite::params![root, MAX_SAMPLES], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(sql_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sql_error)?;
    let mut out: Vec<Sample> = rows
        .into_iter()
        .filter_map(|(finished_at_ns, json)| {
            let v: Value = serde_json::from_str(&json).ok()?;
            Some(Sample {
                finished_at_ns,
                logical_bytes: v.get("logical_bytes")?.as_u64()?,
                complete: v.get("complete").and_then(Value::as_bool) == Some(true),
            })
        })
        .collect();
    out.reverse();
    Ok(out)
}

impl Inner {
    /// Returns the report with the `(catalog_rev, state_rev)` of the snapshot it was read from.
    fn alerts_report(
        &self,
        work: Option<&ReadWork>,
    ) -> Result<(AlertReport, (i64, i64)), ErrorBody> {
        let deadline = work.map_or_else(|| Instant::now() + Duration::from_secs(2), |w| w.deadline);
        let input = Input::open(self, deadline, work)?;
        let settings = settings(&input.conn)?;
        // Volumes that hold an active grant, in the same observation `volumes.list` reports.
        let granted: Vec<i64> = input
            .conn
            .prepare_cached(
                "SELECT DISTINCT r.volume_id FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id
                 WHERE g.state='active' AND r.volume_id IS NOT NULL ORDER BY r.volume_id",
            )
            .map_err(sql_error)?
            .query_map([], |r| r.get(0))
            .map_err(sql_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(sql_error)?;
        let volumes: Vec<VolumeInput> = input
            .volumes(deadline, true)?
            .into_iter()
            .filter_map(|o| {
                let row: i64 = o.volume.volume_id.strip_prefix("vo_")?.parse().ok()?;
                granted.contains(&row).then(|| VolumeInput {
                    volume_id: VolumeId::new(self.volume_id(row)).expect("sealed volume id"),
                    label: o.volume.display_name.to_string(),
                    online: o.volume.online,
                    capacity_bytes: o.volume.capacity_bytes.map(Bytes::get),
                    free_bytes: o.volume.free_bytes.map(Bytes::get),
                })
            })
            .collect();
        let mut roots = Vec::new();
        for r in db::roots(&input.conn, None)
            .map_err(sql_error)?
            .into_iter()
            .filter(|r| r.grant_state == "active")
        {
            roots.push(RootInput {
                root_id: RootId::new(self.root_id(r.root)).expect("sealed root id"),
                label: r.display_path.chars().take(64).collect(),
                samples: samples(&input.conn, r.root)?,
            });
        }
        Ok((
            alerts::evaluate(&settings, &volumes, &roots, now_ns()),
            input.revisions,
        ))
    }

    fn alerts_out(&self, report: AlertReport, revs: (i64, i64)) -> Handled {
        Ok(Out {
            result: serde_json::to_value(report)
                .map_err(|_| fail(ErrorCode::InternalError, "encode alerts"))?,
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }

    pub(crate) fn alerts_list(&self, work: Option<&ReadWork>) -> Handled {
        let (report, revs) = self.alerts_report(work)?;
        self.alerts_out(report, revs)
    }

    /// Replaces the whole threshold record (every field is required), then answers with the
    /// re-evaluated report. Re-sending the same record is harmless.
    pub(crate) fn alerts_configure(&self, s: AlertSettings) -> Handled {
        let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let json = serde_json::to_string(&s)
            .map_err(|_| fail(ErrorCode::InternalError, "encode alert settings"))?;
        let state_rev = db::set_alert_settings(&mut self.db(), &json).map_err(sql_error)?;
        drop(_m);
        let (report, revs) = self.alerts_report(None)?;
        self.alerts_out(report, (revs.0, state_rev))
    }
}
