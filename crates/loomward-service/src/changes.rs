//! #1092: `changes.new_since`. What first appeared in one active granted root since a baseline scan run,
//! from the catalogue's `born_run` columns: the reader supplies the rows, this module words them for the
//! wire. Owner-only and read-only; no schema change, no live path, no effect. No earlier completed scan
//! is `no_baseline` with null figures, never zero; a partial answer names why (invariant 4).
use crate::{catalog_error, db, fail, sql_error, ts_ns, Inner, Out, ReadWork};
use loomward_catalog::{NewSince, ScanRunInfo};
use loomward_protocol::*;
use serde_json::{json, Value};

const CAVEATS: [(&str, &str); 4] = [
    (
        "moved_files_count_as_new",
        "A file moved to another folder counts as new here: only a move inside one folder keeps its place in the history.",
    ),
    (
        "recreated_files_count_as_new",
        "A file deleted and written again counts as new.",
    ),
    (
        "first_listing_counts_as_new",
        "A folder the earlier scan could not list (denied, depth-limited or partial) counts as new when a later scan first lists it.",
    ),
    (
        "existing_files_not_measured",
        "A file that existed before and grew is not counted: sizes are not kept per scan.",
    ),
];

fn run(r: &ScanRunInfo) -> Value {
    json!({
        "run": r.run.to_string(),
        "mode": r.mode,
        "finished_at": r.finished_at_ns.map(ts_ns),
        "complete": r.complete,
    })
}

fn bytes(b: Option<u64>) -> Value {
    b.map(|b| b.to_string()).into()
}

impl Inner {
    pub(crate) fn changes_new_since(
        &self,
        req: &RequestEnvelope,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        let r: ChangesNewSinceRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let row = self.root_row(&r.root_id)?;
        let (active, generation) = {
            let conn = self.db();
            (
                db::active_root_dir(&conn, row).map_err(sql_error)?,
                db::root_generation(&conn, row).map_err(sql_error)?,
            )
        };
        if active.is_none() {
            return Err(fail(ErrorCode::NotFound, "unknown root"));
        }
        let baseline = r
            .baseline_run
            .map(|g| i64::try_from(g.get()))
            .transpose()
            .map_err(|_| fail(ErrorCode::InvalidRequest, "baseline_run is out of range"))?;
        let stale = self.freshness.stale(row);
        let found: NewSince = self
            .read_reader(work)?
            .new_since(row, baseline, r.limit.get() as usize)
            .map_err(catalog_error)?;
        let mut reasons: Vec<&str> = found.partial_reasons.clone();
        // Unresolved watcher changes since the scan: the figures describe an earlier state.
        if stale && found.totals.is_some() {
            reasons.push("changes_since_scan");
        }
        let mut value = json!({
            "root_id": r.root_id,
            "generation": generation.map(|g| g.to_string()),
            "outcome": if found.totals.is_some() { "ok" } else { "no_baseline" },
            "baseline": found.baseline.as_ref().map(run),
            "latest": found.latest.as_ref().map(run),
            "new_files": null,
            "new_dirs": null,
            "logical_bytes": null,
            "allocated_bytes": null,
            "allocation_unknown_files": null,
            "largest_files": [],
            "largest_folders": [],
            "partial": !reasons.is_empty(),
            "partial_reasons": reasons,
            "caveats": [],
        });
        if let Some(t) = &found.totals {
            value["new_files"] = t.new_files.into();
            value["new_dirs"] = t.new_dirs.into();
            value["logical_bytes"] = bytes(Some(t.logical_bytes));
            value["allocated_bytes"] = bytes(t.allocated_bytes);
            value["allocation_unknown_files"] = t.allocation_unknown_files.into();
            value["largest_files"] = found
                .largest_files
                .iter()
                .map(|f| {
                    json!({
                        "node_id": f.node.reference(),
                        "name": f.name,
                        "path": {"text": f.path.text, "truncated": f.path.truncated},
                        "logical_bytes": f.logical_bytes.to_string(),
                        "allocated_bytes": bytes(f.allocated_bytes),
                    })
                })
                .collect();
            value["largest_folders"] = found
                .largest_folders
                .iter()
                .map(|f| {
                    json!({
                        "node_id": f.node.reference(),
                        "name": f.name,
                        "path": {"text": f.path.text, "truncated": f.path.truncated},
                        "logical_bytes": bytes(f.logical_bytes),
                        "allocated_bytes": bytes(f.allocated_bytes),
                        "coverage": crate::clutter::coverage(&f.coverage),
                    })
                })
                .collect();
            value["caveats"] = CAVEATS
                .iter()
                .map(|(topic, note)| json!({"topic": topic, "note": note}))
                .collect();
        }
        // One SQLite snapshot answered; a scan running meanwhile is reported as `scan_running`, not refused.
        self.seal(&mut value)?;
        Ok(Out {
            result: value,
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }
}
