//! LW-131 R1d: owner-only catalogue advice. No cleanup, link-count pass or execute path.
use crate::{catalog_error, db, fail, placement::Input, sql_error, Inner, Out, ReadWork};
use loomward_catalog::{DeveloperClutter, MachineStores, NodeKey};
use loomward_engine::reclaim::{self, ReclaimInput};
use loomward_protocol::*;
use serde_json::json;
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn root_path(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

impl Inner {
    pub(crate) fn reclaim_plan(
        &self,
        req: &RequestEnvelope,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        let r: ReclaimPlanRequest = req.decode_payload()?;
        let deadline = work.map_or_else(|| Instant::now() + Duration::from_secs(2), |w| w.deadline);
        let input = Input::open(self, deadline, work)?;
        let volume = db::volumes(&input.conn, None)
            .map_err(sql_error)?
            .into_iter()
            .find(|v| v.key == r.volume_key.as_str())
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?;
        let observed = input
            .volumes(deadline, true)?
            .into_iter()
            .find(|v| v.volume.volume_id.as_str() == format!("vo_{}", volume.id))
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?;
        let mut roots: Vec<_> = db::roots(&input.conn, None)
            .map_err(sql_error)?
            .into_iter()
            .filter(|r| r.grant_state == "active" && r.volume == Some(volume.id))
            .collect();
        let paths: Vec<_> = roots
            .iter()
            .map(|r| (r.root, root_path(&r.display_path)))
            .collect();
        roots.retain(|r| {
            let path = root_path(&r.display_path);
            !paths.iter().any(|(id, outer)| {
                *id != r.root
                    && (path.starts_with(&format!("{outer}\\")) || (path == *outer && *id < r.root))
            })
        });
        // Like placement: with no active grant there is nothing to read, and an empty plan would read as nothing found.
        if roots.is_empty() {
            return Err(fail(ErrorCode::InvalidRequest, "no_active_grant"));
        }
        // Offline also covers a foreign disk on the root's letter (#910): its bin is not this volume's.
        let online = observed.volume.online;
        let mut planner = ReclaimInput {
            volume: reclaim::ReclaimVolume {
                volume_key: volume.key,
                label: observed.volume.label.map(|s| s.as_str().to_owned()),
                bus_type: (observed.volume.device.bus_type != DeviceHintBusType::Unknown).then(
                    || {
                        serde_json::to_value(observed.volume.device.bus_type)
                            .expect("bus type")
                            .as_str()
                            .expect("bus word")
                            .to_owned()
                    },
                ),
                capacity_bytes: observed.volume.capacity_bytes.map(Bytes::get),
                free_bytes: observed.volume.free_bytes.map(Bytes::get),
                // Placement's zero is an assumption; no reserve preference is recorded yet.
                reserve_bytes: None,
                volume_serial: observed.volume_serial,
                ntfs: volume
                    .filesystem
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("NTFS")),
            },
            limit: r.limit.get() as usize,
            // ponytail: #939 adds the bounded per-candidate link-count pass; until then relief is unknown.
            files: Default::default(),
            ..Default::default()
        };
        let mut reader = self.read_reader(work)?.without_repairs();
        let mut budget_hit = false;
        let mut bin_unknown = false;
        for root in roots {
            let mut coverage = "unscanned".to_owned();
            let mut allocated = None;
            if let Some(dir) = root.dir {
                match reader.inspect(NodeKey::Dir(dir)) {
                    Ok(detail) => {
                        coverage = detail.row.coverage;
                        if coverage == "complete" && root.subtree_complete {
                            allocated = detail.row.allocated_bytes.and_then(|s| s.parse().ok());
                        }
                    }
                    Err(loomward_catalog::Error::ResourceBudget) => {
                        coverage = "unknown".into();
                        budget_hit = true;
                    }
                    Err(e) => return Err(catalog_error(e)),
                }
            }
            if coverage == "complete" && root.state != "complete" {
                coverage = "partial".into();
                allocated = None;
            }
            let stale = self.freshness.stale(root.root) || coverage == "stale";
            if stale {
                coverage = "stale".into();
                allocated = None;
            }
            let mut developer = match reader.developer_clutter(root.root, planner.limit) {
                Ok(found) => found,
                Err(loomward_catalog::Error::ResourceBudget) => {
                    budget_hit = true;
                    DeveloperClutter {
                        rules_version: 1,
                        partial: true,
                        ..Default::default()
                    }
                }
                Err(e) => return Err(catalog_error(e)),
            };
            let mut machine = match reader.machine_stores(root.root, planner.limit) {
                Ok(found) => found,
                Err(loomward_catalog::Error::ResourceBudget) => MachineStores {
                    rules_version: reclaim::RULES_VERSION,
                    partial: true,
                    budget_hit: true,
                    ..Default::default()
                },
                Err(e) => return Err(catalog_error(e)),
            };
            developer.partial |= coverage != "complete";
            machine.partial |= coverage != "complete";
            if stale {
                for c in &mut developer.candidates {
                    c.logical_bytes = None;
                    c.allocated_bytes = None;
                    c.coverage = "stale".into();
                }
                for c in &mut machine.candidates {
                    c.logical_bytes = None;
                    c.allocated_bytes = None;
                    c.coverage = "stale".into();
                }
            }
            planner.covered_roots.push(reclaim::CoveredRoot {
                root_id: root.root,
                coverage,
                allocated_bytes: allocated,
            });
            planner.developer.push(developer);
            planner.machine.push(machine);
            let path = Path::new(&root.display_path);
            // shortcut: mounted-folder roots stay unknown until volume mount points are recorded.
            if path.is_absolute() && path.parent().is_none() {
                planner.recycle_bin = online
                    .then(|| loomward_windows::recycle_bin::query(path))
                    .flatten();
                // An unread bin is unknown, not empty.
                bin_unknown = planner.recycle_bin.is_none();
            }
        }
        let mut plan = reclaim::plan(planner)?;
        plan.budget_hit |= budget_hit;
        plan.partial |= budget_hit || bin_unknown;
        let bytes = |b: Option<u64>| b.map(Bytes);
        let wire = json!({
            "volume": {
                "volume_key": plan.volume.volume_key, "label": plan.volume.label,
                "bus_type": plan.volume.bus_type, "capacity_bytes": bytes(plan.volume.capacity_bytes),
                "free_bytes": bytes(plan.volume.free_bytes), "reserve_bytes": bytes(plan.volume.reserve_bytes),
            },
            "covered_roots": plan.covered_roots.into_iter().map(|r| json!({
                "root_id": self.root_id(r.root_id), "coverage": r.coverage,
                "allocated_bytes": bytes(r.allocated_bytes),
            })).collect::<Vec<_>>(),
            "groups": plan.groups.into_iter().map(|g| json!({
                "kind": g.kind,
                "candidates": g.candidates.into_iter().map(|c| json!({
                    "node_id": c.node_id, "name": c.name, "class": c.class,
                    "provider": c.provider, "kind": c.kind,
                    "logical_bytes": bytes(c.logical_bytes), "allocated_bytes": bytes(c.allocated_bytes),
                    "reclaimable_bytes": bytes(c.reclaimable_bytes), "reclaimable_reason": c.reclaimable_reason,
                    "coverage": c.coverage, "owner_step": c.owner_step, "relocation": c.relocation,
                    "evidence": c.evidence, "uncertainty": c.uncertainty,
                })).collect::<Vec<_>>()
            })).collect::<Vec<_>>(),
            "unaccounted_bytes": bytes(plan.unaccounted_bytes), "rules_version": plan.rules_version,
            "partial": plan.partial, "truncated": plan.truncated, "budget_hit": plan.budget_hit,
        });
        let dto: ReclaimPlan = serde_json::from_value(wire)
            .map_err(|_| fail(ErrorCode::ResourceBudget, "reclaim projection bounds"))?;
        let mut result = serde_json::to_value(dto)
            .map_err(|_| fail(ErrorCode::InternalError, "encode reclaim plan"))?;
        self.seal(&mut result)?;
        Ok(Out {
            result,
            catalog_rev: Some(input.revisions.0),
            state_rev: Some(input.revisions.1),
        })
    }
}
