//! LW-131 R1d: owner-only catalogue advice. No cleanup or execute path.
//! #939 R1g (docs/52 section 6.1): `reclaim.observe`, the owner-started job that runs the
//! metadata-only link-count pass over the plan's candidates and stores one verdict per candidate
//! in `st.reclaim_relief`, which `reclaim.plan` reads while the root's generation still matches.
use crate::{
    catalog_error, db, fail, placement::Input, sql_error, with_reason, Inner, Out, ReadWork,
};
use loomward_catalog::{DeveloperClutter, MachineStores, NodeKey};
use loomward_engine::{
    jobs::{JobContext, JobRunner, JobSpec},
    placement::FileEntry,
    reclaim::{self, CandidateFiles, ReclaimInput, ReclaimVolume},
    scan::CatalogRootIdentity,
    EngineError, EngineResult,
};
use loomward_protocol::*;
use loomward_windows::linkcount::{self, Outcome, Stop, MAX_FILES_PER_GROUP};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

/// Detectors cap all classes together, so they read their maximum; the planner applies the
/// request's limit per group after grouping (Codex on #959).
const DETECTOR_CAP: usize = 200;
/// docs/52 section 6.1: the observation's default wall-clock budget.
const OBSERVE_BUDGET: Duration = Duration::from_secs(600);
/// Names opened between cancellation checks.
const OBSERVE_CHUNK: usize = 4096;

fn root_path(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// The plan's inputs for one volume, read once and shared by `reclaim.plan` and `reclaim.observe`.
struct Gathered<'a> {
    input: Input<'a>,
    volume_row: i64,
    planner: ReclaimInput,
    /// Outermost active roots, index-aligned with `planner.covered_roots`, `developer`, `machine`.
    roots: Vec<db::RootRow>,
    budget_hit: bool,
    online: bool,
}

/// One candidate the observation job visits, in the plan's terms.
struct Task {
    root: usize,
    node: NodeKey,
    class: &'static str,
    logical: u64,
    allocated: u64,
}

/// A root the job may open under: its grant, catalogue row, generation and verified path.
struct ObservedRoot {
    grant: i64,
    row: i64,
    generation: i64,
    path: PathBuf,
}

struct Observation {
    volume_row: i64,
    volume: ReclaimVolume,
    roots: Vec<Option<ObservedRoot>>,
    tasks: Vec<Task>,
    budget: Duration,
}

impl Inner {
    fn reclaim_gather(
        &self,
        volume_id: &str,
        limit: usize,
        deadline: Instant,
        work: Option<&ReadWork>,
    ) -> Result<Gathered<'_>, ErrorBody> {
        let input = Input::open(self, deadline, work)?;
        let volume_row = self.volume_row(volume_id)?;
        let volume = db::volumes(&input.conn, None)
            .map_err(sql_error)?
            .into_iter()
            .find(|v| v.id == volume_row)
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
            limit,
            ..Default::default()
        };
        let mut reader = self.read_reader(work)?.without_repairs();
        let mut budget_hit = false;
        for root in &roots {
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
            let mut developer = match reader.developer_clutter(root.root, DETECTOR_CAP) {
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
            let mut machine = match reader.machine_stores(root.root, DETECTOR_CAP) {
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
        }
        Ok(Gathered {
            input,
            volume_row,
            planner,
            roots,
            budget_hit,
            online,
        })
    }

    pub(crate) fn reclaim_plan(
        &self,
        req: &RequestEnvelope,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        let r: ReclaimPlanRequest = req.decode_payload()?;
        let deadline = work.map_or_else(|| Instant::now() + Duration::from_secs(2), |w| w.deadline);
        let Gathered {
            input,
            volume_row,
            mut planner,
            roots,
            budget_hit,
            online,
        } = self.reclaim_gather(r.volume_id.as_str(), r.limit.get() as usize, deadline, work)?;
        let mut bin_unknown = false;
        for (i, root) in roots.iter().enumerate() {
            // A stale root's sizes are unknown, so a stored verdict for it is not shown either.
            if planner.covered_roots[i].coverage != "stale" {
                let observed = self.observed_relief(&input.conn, root.root)?;
                planner.observed.extend(observed);
            }
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
                "volume_id": format!("vo_{volume_row}"), "label": plan.volume.label,
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

    /// Verified relief stored for this root's current generation, catalogue instance and node
    /// incarnations; any other row is not shown, so its candidate stays `RELIEF_UNOBSERVED`.
    fn observed_relief(
        &self,
        conn: &Connection,
        root: i64,
    ) -> Result<Vec<(NodeKey, u64)>, ErrorBody> {
        let mut stmt = conn
            .prepare_cached(
                "SELECT o.node_kind,o.node_id,o.unique_allocated_bytes FROM st.reclaim_relief o
                 JOIN main.root r ON r.grant_id=o.grant_id
                 WHERE r.id=?1 AND o.catalog_instance=?2 AND o.generation=r.generation
                   AND o.unique_allocated_bytes IS NOT NULL
                   AND o.incarnation IS (CASE o.node_kind WHEN 'dir' THEN (SELECT born_run FROM main.dir WHERE id=o.node_id)
                                         ELSE (SELECT born_run FROM main.file WHERE id=o.node_id) END)",
            )
            .map_err(sql_error)?;
        let rows = stmt
            .query_map(params![root, self.instance], |r| {
                let id: i64 = r.get(1)?;
                let node = if r.get::<_, String>(0)? == "dir" {
                    NodeKey::Dir(id)
                } else {
                    NodeKey::File(id)
                };
                Ok((node, r.get::<_, i64>(2)? as u64))
            })
            .map_err(sql_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_error);
        rows
    }

    /// Whether this process runs with an elevated token; the observation refuses one (invariant 2).
    fn observe_elevated(&self) -> bool {
        #[cfg(test)]
        if self
            .hooks
            .reclaim_elevated
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return true;
        }
        #[cfg(windows)]
        return loomward_windows::enumerate::running_elevated();
        #[cfg(not(windows))]
        false
    }

    pub(crate) fn reclaim_observe(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let r: ReclaimObserveRequest = req.decode_payload()?;
        if self.observe_elevated() {
            return Err(with_reason(
                fail(ErrorCode::PermissionDenied, "non-elevated access required"),
                "elevated_token",
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let gathered = self.reclaim_gather(r.volume_id.as_str(), 200, deadline, None)?;
        let planner = &gathered.planner;
        let mut seen = HashSet::new();
        let mut tasks = Vec::new();
        for (i, covered) in planner.covered_roots.iter().enumerate() {
            if covered.coverage == "stale" {
                continue;
            }
            let developer = planner.developer[i].candidates.iter().map(|c| {
                (
                    c.directory,
                    "project_clutter",
                    c.logical_bytes,
                    c.allocated_bytes,
                )
            });
            let machine = planner.machine[i]
                .candidates
                .iter()
                .map(|c| (c.node, c.class, c.logical_bytes, c.allocated_bytes));
            // Same order and de-duplication as the plan, so a node is observed as the plan shows it.
            for (node, class, logical, allocated) in developer.chain(machine) {
                if !seen.insert(node) {
                    continue;
                }
                // The plan states these without a pass: no name is opened for them.
                let (Some(logical), Some(allocated)) = (logical, allocated) else {
                    continue;
                };
                if matches!(class, "virtual_disk" | "system_managed") {
                    continue;
                }
                tasks.push(Task {
                    root: i,
                    node,
                    class,
                    logical,
                    allocated,
                });
            }
        }
        // Largest allocation first: an interrupted job has covered the ones that matter most.
        tasks.sort_by_key(|t| (std::cmp::Reverse(t.allocated), t.node.reference()));
        let mut roots = Vec::with_capacity(gathered.roots.len());
        for (i, root) in gathered.roots.iter().enumerate() {
            let Some(generation) = root
                .generation
                .filter(|_| tasks.iter().any(|t| t.root == i))
            else {
                roots.push(None);
                continue;
            };
            // The same admission as a scan or a duplicate inspection before any native open.
            self.session_root_gate(root)
                .map_err(|e| self.root_admission_error(root, e))?;
            let id = RootId::new(self.root_id(root.root)).expect("rt_ id");
            let granted = self.engine.granted_root(
                id.clone(),
                root.grant,
                &CatalogRootIdentity {
                    row: root.root,
                    instance: self.instance.clone(),
                },
            );
            if matches!(&granted, Err(EngineError::PermissionDenied { message })
                if message == "root_identity_changed")
            {
                self.identity_changed(root.grant, &id, true);
            }
            roots.push(Some(ObservedRoot {
                grant: root.grant,
                row: root.root,
                generation,
                path: granted?.path().to_path_buf(),
            }));
        }
        let observation = Observation {
            volume_row: gathered.volume_row,
            volume: gathered.planner.volume.clone(),
            roots,
            tasks,
            budget: self.observe_budget(),
        };
        drop(gathered);
        let weak = self.weak.clone();
        let runner: JobRunner = Arc::new(move |ctx, _| observe(&weak, &observation, &ctx));
        let job = self
            .engine
            .job_submit_runner(JobSpec::new(JobKind::Reclaim, None), runner)?;
        Ok(Out {
            result: json!({ "job": job }),
            catalog_rev: None,
            state_rev: None,
        })
    }

    /// Forgetting, revoking or losing a root's identity deletes its relief verdicts.
    pub(crate) fn drop_reclaim_relief(&self, grant: i64) -> Result<(), ErrorBody> {
        self.db()
            .execute("DELETE FROM st.reclaim_relief WHERE grant_id=?1", [grant])
            .map(drop)
            .map_err(sql_error)
    }

    fn observe_budget(&self) -> Duration {
        #[cfg(test)]
        if let Some(budget) = *self.hooks.reclaim_budget.lock().unwrap() {
            return budget;
        }
        OBSERVE_BUDGET
    }
}

fn internal(e: impl std::fmt::Display) -> EngineError {
    EngineError::Internal {
        message: format!("reclaim observation: {e}"),
        detail: None,
    }
}

fn gone() -> EngineError {
    EngineError::Internal {
        message: "service stopped".into(),
        detail: None,
    }
}

/// The job body. It holds the service only per candidate, so a stopping service is not kept alive.
fn observe(weak: &Weak<Inner>, job: &Observation, ctx: &JobContext) -> EngineResult<CoverageState> {
    let deadline = Instant::now() + job.budget;
    let conn = db::open(&weak.upgrade().ok_or_else(gone)?.state_dir).map_err(internal)?;
    conn.execute_batch("PRAGMA query_only=ON")
        .map_err(internal)?;
    let count = |n: usize| Count::new(n as u64).map_err(internal);
    let (mut done, mut names, mut complete) = (0, 0, true);
    let progress = |done: usize, names: usize| {
        ctx.set_reclaim_progress(ReclaimObserveProgress {
            candidates_total: count(job.tasks.len())?,
            candidates_done: count(done)?,
            names_checked: count(names)?,
        })
    };
    progress(0, 0)?;
    for task in &job.tasks {
        #[cfg(test)]
        weak.upgrade()
            .ok_or_else(gone)?
            .hooks
            .gate(Command::ReclaimObserve, "candidate");
        if ctx.is_cancelled() {
            return Ok(CoverageState::Cancelled);
        }
        // Unfinished candidates keep no row: their relief stays unknown, never zero.
        if Instant::now() >= deadline {
            complete = false;
            break;
        }
        let Some(root) = &job.roots[task.root] else {
            complete = false;
            continue;
        };
        let inner = weak.upgrade().ok_or_else(gone)?;
        match observe_one(&conn, &inner, job, root, task, deadline, ctx)? {
            Some(checked) => names += checked,
            None => complete = false,
        }
        if ctx.is_cancelled() {
            return Ok(CoverageState::Cancelled);
        }
        done += 1;
        progress(done, names)?;
    }
    Ok(if complete {
        CoverageState::Complete
    } else {
        CoverageState::Partial
    })
}

/// One listed name: catalogue row, path below the candidate, sizes, identity and attributes.
type Listed = (
    i64,
    String,
    u64,
    Option<u64>,
    Option<Vec<u8>>,
    Option<i64>,
    Option<u32>,
    u32,
);

/// Observe one candidate and store its verdict; `None` when it was not finished or not stored.
fn observe_one(
    conn: &Connection,
    inner: &Inner,
    job: &Observation,
    root: &ObservedRoot,
    task: &Task,
    deadline: Instant,
    ctx: &JobContext,
) -> EngineResult<Option<usize>> {
    let Some((volume_key, incarnation, base, listed)) = list(conn, root, task)? else {
        return Ok(None);
    };
    let mut stop = (listed.len() > MAX_FILES_PER_GROUP).then_some(Stop::FileBudget);
    let mut outcomes = Vec::new();
    if stop.is_none() {
        let entries: Vec<_> = listed
            .iter()
            .map(
                |(_, rel, _, _, id, created, reparse, attrs)| linkcount::Entry {
                    path: if rel.is_empty() {
                        base.clone()
                    } else {
                        base.join(rel)
                    },
                    expected: crate::placement::catalogue_object(
                        &volume_key,
                        id.clone(),
                        *created,
                        *reparse,
                    ),
                    attributes: *attrs,
                },
            )
            .collect();
        for chunk in entries.chunks(OBSERVE_CHUNK) {
            if ctx.is_cancelled() {
                return Ok(None);
            }
            let report = linkcount::observe(chunk, deadline);
            outcomes.extend(report.outcomes);
            if report.stop.is_some() {
                stop = report.stop;
                break;
            }
        }
    }
    let checked = outcomes.len();
    let mut outcomes = outcomes.into_iter();
    let files = listed
        .into_iter()
        .map(
            |(id, _, logical, allocated, file_id, created, reparse, _)| FileEntry {
                node_id: NodeId::new(format!("nd_file_{id}")).expect("file row"),
                logical_bytes: logical,
                allocated_bytes: allocated,
                object: crate::placement::catalogue_object(&volume_key, file_id, created, reparse),
                link_observation: match outcomes.next() {
                    Some(Outcome::Matched(o)) => Some(*o),
                    _ => None,
                },
            },
        )
        .collect();
    let (bytes, reason) = reclaim::reclaimable(
        task.class,
        Some(task.logical),
        Some(task.allocated),
        Some(&CandidateFiles { files, stop }),
        None,
        &job.volume,
    )?;
    let (kind, id) = node_parts(task.node);
    // Written only while the grant is active at the generation the names were read under, so a
    // revocation or rescan that lands first leaves no row behind.
    inner
        .db()
        .execute(
            "INSERT INTO st.reclaim_relief(grant_id,node_kind,node_id,volume_id,generation,incarnation,catalog_instance,observed_at_ns,unique_allocated_bytes,reason)
             SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10 WHERE EXISTS(SELECT 1 FROM st.root_grant g JOIN main.root r ON r.grant_id=g.id
                WHERE g.id=?1 AND g.state='active' AND r.id=?11 AND r.generation=?5)
             ON CONFLICT(grant_id,node_kind,node_id) DO UPDATE SET volume_id=excluded.volume_id,generation=excluded.generation,
                incarnation=excluded.incarnation,catalog_instance=excluded.catalog_instance,observed_at_ns=excluded.observed_at_ns,
                unique_allocated_bytes=excluded.unique_allocated_bytes,reason=excluded.reason",
            params![
                root.grant,
                kind,
                id,
                job.volume_row,
                root.generation,
                incarnation,
                inner.instance,
                crate::now_ns(),
                bytes.map(|b| b as i64),
                reason,
                root.row,
            ],
        )
        .map_err(internal)?;
    Ok(Some(checked))
}

fn node_parts(node: NodeKey) -> (&'static str, i64) {
    match node {
        NodeKey::File(id) => ("file", id),
        NodeKey::Dir(id) => ("dir", id),
        _ => unreachable!("reclaim candidates are directories or files"),
    }
}

/// The candidate's names in one read snapshot, at most one past the cap, with the volume key,
/// the node's incarnation and the candidate's path under the granted root. `None` when the root
/// moved on (revoked, rescanned, scanning or partial) or the candidate's path cannot be built.
#[allow(clippy::type_complexity)]
fn list(
    conn: &Connection,
    root: &ObservedRoot,
    task: &Task,
) -> EngineResult<Option<(String, i64, PathBuf, Vec<Listed>)>> {
    conn.execute_batch("BEGIN").map_err(internal)?;
    let read = || -> rusqlite::Result<Option<(String, i64, PathBuf, Vec<Listed>)>> {
        let current: Option<String> = conn
            .query_row(
                "SELECT v.volume_key FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id JOIN main.volume v ON v.id=r.volume_id
                 WHERE r.id=?1 AND g.id=?2 AND g.state='active' AND r.generation=?3 AND r.active_run IS NULL AND r.state='complete'",
                params![root.row, root.grant, root.generation],
                |r| r.get(0),
            )
            .optional()?;
        let Some(volume_key) = current else {
            return Ok(None);
        };
        let (kind, id) = node_parts(task.node);
        let (incarnation, dir, file_name): (i64, i64, Option<String>) = if kind == "dir" {
            conn.query_row(
                "SELECT born_run,id,NULL FROM main.dir WHERE id=?1 AND root_id=?2",
                params![id, root.row],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        } else {
            conn.query_row(
                "SELECT f.born_run,f.dir_id,f.name FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE f.id=?1 AND d.root_id=?2",
                params![id, root.row],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        }?;
        // Ancestors from the candidate up to the root row, whose path is the granted root's.
        let mut stmt = conn.prepare_cached(
            "WITH RECURSIVE a(id,parent_id,name,depth) AS (SELECT id,parent_id,name,0 FROM main.dir WHERE id=?1
             UNION ALL SELECT d.id,d.parent_id,d.name,a.depth+1 FROM main.dir d JOIN a ON d.id=a.parent_id WHERE a.depth<511)
             SELECT parent_id IS NULL,name FROM a ORDER BY depth DESC",
        )?;
        let chain = stmt
            .query_map([dir], |r| {
                Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !chain.first().is_some_and(|(top, _)| *top) {
            return Ok(None);
        }
        let mut base = root.path.clone();
        for (_, name) in chain.iter().skip(1) {
            base.push(name);
        }
        let row = |r: &rusqlite::Row| -> rusqlite::Result<Listed> {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, i64>(2)?.max(0) as u64,
                r.get::<_, Option<i64>>(3)?
                    .and_then(|n| u64::try_from(n).ok()),
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get::<_, Option<u32>>(7)?.unwrap_or(0),
            ))
        };
        let listed = if let Some(name) = file_name {
            base.push(name);
            conn.prepare_cached(
                "SELECT id,'',logical,allocated,file_id,created_ft,reparse_tag,attrs FROM main.file WHERE id=?1 AND flags & ?2=0",
            )?
            .query_map(params![id, loomward_catalog::EXCLUDED_FLAGS], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            // placement `files()`'s visibility rules, with each name's path below the candidate.
            conn.prepare_cached(
                "WITH RECURSIVE visible(id,rel) AS (SELECT id,'' FROM main.dir WHERE id=?1 AND listing_state NOT IN ('excluded','absent_pending') AND flags & ?2=0
                 UNION ALL SELECT d.id,v.rel||d.name||char(92) FROM main.dir d JOIN visible v ON d.parent_id=v.id WHERE d.listing_state NOT IN ('excluded','absent_pending') AND d.flags & ?2=0)
                 SELECT f.id,v.rel||f.name,f.logical,f.allocated,f.file_id,f.created_ft,f.reparse_tag,f.attrs FROM main.file f JOIN visible v ON v.id=f.dir_id
                 WHERE f.flags & ?2=0 ORDER BY f.id LIMIT ?3",
            )?
            .query_map(
                params![id, loomward_catalog::EXCLUDED_FLAGS, MAX_FILES_PER_GROUP as i64 + 1],
                row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(Some((volume_key, incarnation, base, listed)))
    };
    let result = read();
    conn.execute_batch("COMMIT").map_err(internal)?;
    match result {
        Ok(found) => Ok(found),
        // The candidate's row is gone: the catalogue moved on since the job started.
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(internal(e)),
    }
}
