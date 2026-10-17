//! LW-119: `clutter.developer`. Advice from recorded names and aggregates only, for one active granted
//! root: the detector in `loomward-catalog` supplies the candidates, this module words them for the wire.
//! Read-only and owner-only (a client principal is refused before dispatch). A detector that hits its
//! work budget is reported as such, never as an empty list; unknown stays unknown (invariant 4).
use crate::{catalog_error, db, fail, sql_error, Inner, Out, ReadWork};
use loomward_protocol::*;
use serde_json::{json, Value};

type Handled = Result<Out, ErrorBody>;

const NOT_ASSESSED: [(&str, &str); 3] = [
    (
        "regeneration_cost",
        "Rebuild cost and time are not measured yet (LW-087).",
    ),
    (
        "model_host_lifecycle",
        "Model files and model-host state are not assessed yet (LW-086).",
    ),
    (
        "stale_worktrees",
        "Stale worktrees need lease and dirty-state evidence, which is not collected yet (LW-076).",
    ),
];

/// The wire `CoverageState` word for a catalogue coverage string; anything else reads unknown.
fn coverage(c: &str) -> &str {
    match c {
        "complete" | "partial" | "stale" | "unscanned" | "denied" | "excluded" | "cancelled" => c,
        _ => "unknown",
    }
}

impl Inner {
    pub(crate) fn clutter_developer(
        &self,
        req: &RequestEnvelope,
        work: Option<&ReadWork>,
    ) -> Handled {
        let r: DeveloperClutterRequest = req.decode_payload()?;
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
        let mut reader = self.read_reader(work)?;
        let stale = self.freshness.stale(row);
        let mut report = json!({
            "root_id": self.root_id(row),
            "generation": generation.map(|g| g.to_string()),
            "rules_version": 1,
            "partial": true,
            "truncated": false,
            "budget_hit": false,
            "budget_note": null,
            "candidates": [],
            "not_assessed": NOT_ASSESSED.iter().map(|(topic, note)| json!({"topic": topic, "note": note})).collect::<Vec<_>>(),
        });
        match reader.developer_clutter(row, r.limit.get() as usize) {
            Ok(found) => {
                report["rules_version"] = found.rules_version.into();
                // A root whose watcher saw unresolved changes may have moved since the scan: its sizes
                // are unknown now, not the last recorded ones.
                report["partial"] = (found.partial || stale).into();
                report["truncated"] = found.truncated.into();
                report["candidates"] = Value::Array(
                    found
                        .candidates
                        .into_iter()
                        .map(|c| {
                            let size = |b: Option<u64>| {
                                if stale {
                                    None
                                } else {
                                    b.map(|b| b.to_string())
                                }
                            };
                            json!({
                                "node_id": c.directory.reference(),
                                "name": c.directory_name,
                                "anchor_node_id": c.anchor.reference(),
                                "anchor_name": c.anchor_name,
                                "provider": c.provider,
                                "kind": c.kind,
                                "logical_bytes": size(c.logical_bytes),
                                "allocated_bytes": size(c.allocated_bytes),
                                "coverage": if stale { "stale" } else { coverage(&c.coverage) },
                                "regeneration_cost": c.regeneration_cost,
                                "regeneration_hint": c.regeneration_hint,
                                "reason": c.reason,
                                "uncertainty": c.uncertainty,
                            })
                        })
                        .collect(),
                );
            }
            Err(loomward_catalog::Error::ResourceBudget) => {
                report["budget_hit"] = true.into();
                report["budget_note"] = "The search reached its work budget before it finished. No candidates are listed, and that says nothing about what this folder holds.".into();
            }
            Err(e) => return Err(catalog_error(e)),
        }
        let mut value = report;
        self.seal(&mut value)?;
        Ok(Out {
            result: value,
            catalog_rev: Some(revs.0),
            state_rev: Some(revs.1),
        })
    }
}
