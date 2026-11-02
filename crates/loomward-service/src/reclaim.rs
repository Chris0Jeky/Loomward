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
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

pub(crate) mod nested;
mod relief;
mod selection;

/// Detectors cap all classes together, so they read their maximum; the planner applies the
/// request's limit per group after grouping (Codex on #959).
const DETECTOR_CAP: usize = 200;
/// docs/52 section 6.1: the observation's default wall-clock budget.
const OBSERVE_BUDGET: Duration = Duration::from_secs(600);
/// Names opened between cancellation and grant checks.
const OBSERVE_CHUNK: usize = 1000;

fn root_path(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// The outermost active roots on a volume, so covered allocation is never counted twice.
fn active_roots(conn: &Connection, volume: i64) -> Result<Vec<db::RootRow>, ErrorBody> {
    let mut roots: Vec<_> = db::roots(conn, None)
        .map_err(sql_error)?
        .into_iter()
        .filter(|r| r.grant_state == "active" && r.volume == Some(volume))
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
    Ok(roots)
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
    logical: Option<u64>,
    allocated: Option<u64>,
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
    selection: Option<Vec<selection::Binding>>,
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
            .volumes(deadline, true, false)?
            .into_iter()
            .find(|v| v.volume.volume_id.as_str() == format!("vo_{}", volume.id))
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?;
        let roots = active_roots(&input.conn, volume.id)?;
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
                #[cfg(test)]
                let inspect = if self
                    .hooks
                    .reclaim_inspect_budget
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    Err(loomward_catalog::Error::ResourceBudget)
                } else {
                    reader.inspect(NodeKey::Dir(dir))
                };
                #[cfg(not(test))]
                let inspect = reader.inspect(NodeKey::Dir(dir));
                match inspect {
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
        let gathered =
            self.reclaim_gather(r.volume_id.as_str(), r.limit.get() as usize, deadline, work)?;
        let selection = r
            .selection
            .as_ref()
            .map(|ids| selection::read(self, &gathered, ids))
            .transpose()?;
        let Gathered {
            input,
            volume_row,
            mut planner,
            roots,
            budget_hit,
            online,
        } = gathered;
        let mut bin_unknown = false;
        // When each shown verdict was observed (#1029), by the candidate's wire reference.
        let mut observed_at: HashMap<String, i64> = HashMap::new();
        for (i, root) in roots.iter().enumerate() {
            // A stale root's sizes are unknown, so a stored verdict for it is not shown either.
            if planner.covered_roots[i].coverage != "stale" {
                for (node, bytes, at) in self.observed_relief(&input.conn, root.root, None)? {
                    planner.observed.insert(node, bytes);
                    observed_at.insert(node.reference(), at);
                }
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
        let relief = relief::relief(&input.conn, &self.instance, volume_row)?;
        let nodes = nested::references(&planner);
        let mut plan = reclaim::plan(planner)?;
        // A candidate inside another candidate is listed once, under the outer one (#1076).
        nested::dedupe(&input.conn, &mut plan, &nodes)?;
        plan.budget_hit |= budget_hit;
        plan.partial |= budget_hit || bin_unknown;
        let bytes = |b: Option<u64>| b.map(Bytes);
        let mut wire = json!({
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
                    // The plan runs no pass of its own, so known relief is always a stored verdict (#1029).
                    "observed_at": c.reclaimable_bytes.and(c.node_id.as_ref())
                        .and_then(|id| observed_at.get(id)).map(|&ns| crate::ts_ns(ns)),
                    "coverage": c.coverage, "owner_step": c.owner_step, "relocation": c.relocation,
                    "evidence": c.evidence, "uncertainty": c.uncertainty,
                })).collect::<Vec<_>>()
            })).collect::<Vec<_>>(),
            "unaccounted_bytes": bytes(plan.unaccounted_bytes), "rules_version": plan.rules_version,
            "partial": plan.partial, "truncated": plan.truncated, "budget_hit": plan.budget_hit,
        });
        if let Some(selection) = selection {
            wire["selection"] = selection;
        }
        if let Some(relief) = relief {
            wire["relief"] = relief;
        }
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

    /// Verified relief stored for this root's current generation, catalogue instance, node
    /// incarnation and subtree revision, on a complete root with no scan in flight. A targeted or
    /// partial run moves `subtree_rev` without a generation bump (#970 review); any other row is
    /// not shown, so its candidate stays `RELIEF_UNOBSERVED`. Each verdict carries the Unix
    /// nanoseconds it was observed at; the bindings track the catalogue, not the filesystem, so
    /// that age is the only sign a figure may have drifted (#1029). `only` reads one node's row.
    pub(crate) fn observed_relief(
        &self,
        conn: &Connection,
        root: i64,
        only: Option<NodeKey>,
    ) -> Result<Vec<(NodeKey, u64, i64)>, ErrorBody> {
        let (kind, id) = match only {
            Some(NodeKey::Dir(id)) => (Some("dir"), Some(id)),
            Some(NodeKey::File(id)) => (Some("file"), Some(id)),
            _ => (None, None),
        };
        let mut stmt = conn
            .prepare_cached(
                "SELECT o.node_kind,o.node_id,o.unique_allocated_bytes,o.observed_at_ns FROM st.reclaim_relief o
                 JOIN main.root r ON r.grant_id=o.grant_id
                 WHERE r.id=?1 AND o.catalog_instance=?2 AND o.generation=r.generation
                   AND r.state='complete' AND r.active_run IS NULL
                   AND o.unique_allocated_bytes IS NOT NULL
                   AND (?3 IS NULL OR (o.node_kind=?3 AND o.node_id=?4))
                   AND o.incarnation IS (CASE o.node_kind WHEN 'dir' THEN (SELECT born_run FROM main.dir WHERE id=o.node_id)
                                         ELSE (SELECT born_run FROM main.file WHERE id=o.node_id) END)
                   AND o.subtree_rev IS (CASE o.node_kind WHEN 'dir' THEN (SELECT subtree_rev FROM main.dir WHERE id=o.node_id)
                                         ELSE (SELECT d.subtree_rev FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE f.id=o.node_id) END)",
            )
            .map_err(sql_error)?;
        let rows = stmt
            .query_map(params![root, self.instance, kind, id], |r| {
                let id: i64 = r.get(1)?;
                let node = if r.get::<_, String>(0)? == "dir" {
                    NodeKey::Dir(id)
                } else {
                    NodeKey::File(id)
                };
                Ok((node, r.get::<_, i64>(2)? as u64, r.get(3)?))
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
        let selection = r
            .selection
            .as_ref()
            .map(|ids| selection::prepare(self, &gathered, ids))
            .transpose()?;
        let planner = &gathered.planner;
        let mut seen = HashSet::new();
        let mut tasks = Vec::new();
        for (i, covered) in planner.covered_roots.iter().enumerate() {
            if covered.coverage == "stale" && selection.is_none() {
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
                if selection.is_none()
                    && (logical.is_none()
                        || allocated.is_none()
                        || matches!(class, "virtual_disk" | "system_managed"))
                {
                    continue;
                }
                if selection
                    .as_ref()
                    .is_some_and(|members| !members.iter().any(|m| m.node() == node))
                {
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
            // The same admission as a scan or a duplicate inspection before any native open. A root
            // that fails it is skipped (its candidates stay unknown, the job partial); this read job
            // never ends a grant: identity changes stay with scans and freshness (#970 review).
            let id = RootId::new(self.root_id(root.root)).expect("rt_ id");
            let granted = self.session_root_gate(root).ok().and_then(|()| {
                self.engine
                    .granted_root(
                        id,
                        root.grant,
                        &CatalogRootIdentity {
                            row: root.root,
                            instance: self.instance.clone(),
                        },
                    )
                    .ok()
            });
            roots.push(granted.map(|granted| ObservedRoot {
                grant: root.grant,
                row: root.root,
                generation,
                path: verbatim(granted.path()),
            }));
        }
        let observation = Observation {
            volume_row: gathered.volume_row,
            volume: gathered.planner.volume.clone(),
            roots,
            tasks,
            budget: self.observe_budget(),
            selection,
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

    /// Forgetting, revoking or losing a root's identity deletes its relief verdicts, and the relief
    /// samples of a volume left with no active grant (#1013).
    pub(crate) fn drop_reclaim_relief(&self, grant: i64) -> Result<(), ErrorBody> {
        // A coverage report names the files of both its folders, so it ends with either grant (#1106).
        // Before the state lock: `store_coverage` takes the reports lock, then the state lock.
        self.covers.forget_grant(grant);
        let conn = self.db();
        conn.execute("DELETE FROM st.reclaim_selection WHERE EXISTS(SELECT 1 FROM json_each(members) WHERE json_extract(value,'$.grant')=?1)", [grant]).map_err(sql_error)?;
        conn.execute("DELETE FROM st.reclaim_relief WHERE grant_id=?1", [grant])
            .map_err(sql_error)?;
        // The grant's state has already left `active`, so a volume with no other active grant loses its samples.
        conn.execute(db::PURGE_ORPHAN_SAMPLES, [])
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
    if let Some(members) = &job.selection {
        return selection::observe(weak, job, members, ctx);
    }
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

/// One directory to check before opening beneath it: path and catalogue identity.
type ListedDir = (PathBuf, Option<Vec<u8>>, Option<i64>);

/// A candidate's names in one read snapshot, with what binds a stored verdict to it.
struct Listing {
    volume_key: String,
    incarnation: i64,
    subtree_rev: i64,
    /// The candidate's path under the granted root (a verbatim path on Windows).
    base: PathBuf,
    /// Every directory from below the root down through the candidate, parents first.
    dirs: Vec<ListedDir>,
    directory_overflow: bool,
    names: Vec<Listed>,
}

/// A `\\?\` path, so Win32 never reads `com3` or `nul.txt` as a device and long paths open.
pub(crate) fn verbatim(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        if text.starts_with(r"\\?\") {
            return path.to_path_buf();
        }
        if let Some(unc) = text.strip_prefix(r"\\") {
            return PathBuf::from(format!(r"\\?\UNC\{unc}"));
        }
        PathBuf::from(format!(r"\\?\{text}"))
    }
    #[cfg(not(windows))]
    path.to_path_buf()
}

/// Each directory is still the catalogued one and not a reparse point, checked parents first, so
/// no open below a directory swapped for a junction or symlink after the scan reaches elsewhere.
/// ponytail: path-based, so a swap between this check and the opens is not excluded; opening
/// relative to pinned directory handles closes that window but `linkcount` takes paths today.
fn dirs_unchanged(
    volume_key: &str,
    dirs: &[ListedDir],
    deadline: Instant,
    ctx: &JobContext,
) -> bool {
    dirs.iter().enumerate().all(|(i, (path, id, created))| {
        // Cancel and the budget stop the check too, not only the name pass (review of #970).
        if i % 1_000 == 0 && (ctx.is_cancelled() || Instant::now() >= deadline) {
            return false;
        }
        let expected = crate::placement::catalogue_object(volume_key, id.clone(), *created, None);
        loomward_windows::identity::observe(path).is_ok_and(|o| {
            o.attributes & 0x10 != 0
                && o.attributes & 0x400 == 0
                && o.object.reparse_tag == 0
                && expected.is_some_and(|e| {
                    e.file_id == o.object.file_id && e.volume_serial == o.object.volume_serial
                })
        })
    })
}

/// Observe one candidate and store its verdict. `Ok(None)` when it was not finished or failed on
/// its own: it stays unknown with no row. Only a failed store fails the job.
fn observe_one(
    conn: &Connection,
    inner: &Inner,
    job: &Observation,
    root: &ObservedRoot,
    task: &Task,
    deadline: Instant,
    ctx: &JobContext,
) -> EngineResult<Option<usize>> {
    let listing = list(conn, root, task);
    #[cfg(test)]
    let listing = if inner
        .hooks
        .reclaim_fail_candidate
        .swap(false, std::sync::atomic::Ordering::AcqRel)
    {
        Err(internal("injected candidate failure"))
    } else {
        listing
    };
    let Listing {
        volume_key,
        incarnation,
        subtree_rev,
        base,
        dirs,
        directory_overflow,
        names,
    } = match listing {
        Ok(Some(listing)) => listing,
        Ok(None) => return Ok(None),
        Err(e) => {
            eprintln!("loomward-service: reclaim candidate left unknown: {e}");
            return Ok(None);
        }
    };
    // Directories past the cap would go unchecked, so they leave the candidate unknown too.
    let stop =
        (names.len() > MAX_FILES_PER_GROUP || directory_overflow).then_some(Stop::FileBudget);
    let dirs_ok = stop.is_some() || dirs_unchanged(&volume_key, &dirs, deadline, ctx);
    let mut outcomes = Vec::new();
    if stop.is_none() && dirs_ok {
        let entries: Vec<_> = names
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
        for (i, chunk) in entries.chunks(OBSERVE_CHUNK).enumerate() {
            #[cfg(test)]
            if i > 0 {
                inner.hooks.gate(Command::ReclaimObserve, "chunk");
            }
            #[cfg(not(test))]
            let _ = i;
            // The job has no root, so revoking one cannot cancel it: it stops opening here instead.
            let active = conn
                .query_row(
                    "SELECT state='active' FROM st.root_grant WHERE id=?1",
                    [root.grant],
                    |r| r.get::<_, bool>(0),
                )
                .optional()
                .map_err(internal)?
                .unwrap_or(false);
            if ctx.is_cancelled() || !active {
                return Ok(None);
            }
            #[cfg(test)]
            inner
                .hooks
                .reclaim_names_opened
                .fetch_add(chunk.len(), std::sync::atomic::Ordering::AcqRel);
            #[cfg(test)]
            let deadline = inner
                .hooks
                .reclaim_linkcount_deadline
                .lock()
                .unwrap()
                .take()
                .unwrap_or(deadline);
            let report = linkcount::observe(chunk, deadline);
            outcomes.extend(report.outcomes);
            // A budget stop inside the candidate leaves it unfinished: no row, a partial job.
            if report.stop.is_some() {
                return Ok(None);
            }
        }
    }
    let checked = outcomes.len();
    let mut outcomes = outcomes.into_iter();
    let files = names
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
    let verdict = if dirs_ok {
        reclaim::reclaimable(
            task.class,
            task.logical,
            task.allocated,
            Some(&CandidateFiles { files, stop }),
            None,
            &job.volume,
        )
    } else {
        Ok((None, Some(reclaim::RELIEF_UNOBSERVED)))
    };
    let (bytes, reason) = match verdict {
        Ok(verdict) => verdict,
        Err(e) => {
            eprintln!("loomward-service: reclaim candidate left unknown: {e}");
            return Ok(None);
        }
    };
    let (kind, id) = node_parts(task.node);
    // Written only while the grant is active at the generation the names were read under, so a
    // revocation or rescan that lands first leaves no row behind.
    inner
        .db()
        .execute(
            "INSERT INTO st.reclaim_relief(grant_id,node_kind,node_id,volume_id,generation,incarnation,subtree_rev,catalog_instance,observed_at_ns,unique_allocated_bytes,reason)
             SELECT ?1,?2,?3,?4,?5,?6,?12,?7,?8,?9,?10 WHERE EXISTS(SELECT 1 FROM st.root_grant g JOIN main.root r ON r.grant_id=g.id
                WHERE g.id=?1 AND g.state='active' AND r.id=?11 AND r.generation=?5)
             ON CONFLICT(grant_id,node_kind,node_id) DO UPDATE SET volume_id=excluded.volume_id,generation=excluded.generation,
                incarnation=excluded.incarnation,subtree_rev=excluded.subtree_rev,catalog_instance=excluded.catalog_instance,
                observed_at_ns=excluded.observed_at_ns,unique_allocated_bytes=excluded.unique_allocated_bytes,reason=excluded.reason",
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
                subtree_rev,
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

/// placement `files()`'s visibility rules, with each directory's path below the candidate.
const VISIBLE: &str = "WITH RECURSIVE visible(id,rel,depth) AS (SELECT id,'',0 FROM main.dir WHERE id=?1 AND listing_state NOT IN ('excluded','absent_pending') AND flags & ?2=0
     UNION ALL SELECT d.id,v.rel||d.name||char(92),v.depth+1 FROM main.dir d JOIN visible v ON d.parent_id=v.id WHERE d.listing_state NOT IN ('excluded','absent_pending') AND d.flags & ?2=0)";

/// The candidate's names in one read snapshot, at most one past the cap. `None` when the root
/// moved on (revoked, rescanned, scanning or partial) or the candidate's path cannot be built.
fn list(conn: &Connection, root: &ObservedRoot, task: &Task) -> EngineResult<Option<Listing>> {
    list_bounded(
        conn,
        root,
        task,
        MAX_FILES_PER_GROUP,
        MAX_FILES_PER_GROUP,
        &HashSet::new(),
    )
}

fn list_bounded(
    conn: &Connection,
    root: &ObservedRoot,
    task: &Task,
    name_limit: usize,
    dir_limit: usize,
    seen_dirs: &HashSet<PathBuf>,
) -> EngineResult<Option<Listing>> {
    conn.execute_batch("BEGIN").map_err(internal)?;
    let read = || -> rusqlite::Result<Option<Listing>> {
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
        // Any write under the candidate's directory moves it, with or without a generation bump.
        let subtree_rev: i64 =
            conn.query_row("SELECT subtree_rev FROM main.dir WHERE id=?1", [dir], |r| {
                r.get(0)
            })?;
        // Ancestors from the candidate up to the root row, whose path is the granted root's.
        let chain = conn
            .prepare_cached(
                "WITH RECURSIVE a(id,parent_id,name,file_id,created_ft,depth) AS (SELECT id,parent_id,name,file_id,created_ft,0 FROM main.dir WHERE id=?1
                 UNION ALL SELECT d.id,d.parent_id,d.name,d.file_id,d.created_ft,a.depth+1 FROM main.dir d JOIN a ON d.id=a.parent_id WHERE a.depth<511)
                 SELECT parent_id IS NULL,name,file_id,created_ft FROM a ORDER BY depth DESC",
            )?
            .query_map([dir], |r| {
                Ok((
                    r.get::<_, bool>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<Vec<u8>>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !chain.first().is_some_and(|(top, ..)| *top) {
            return Ok(None);
        }
        let mut base = root.path.clone();
        let mut dirs = Vec::new();
        for (_, name, file_id, created) in chain.into_iter().skip(1) {
            base.push(name);
            dirs.push((base.clone(), file_id, created));
        }
        let new_ancestors = dirs
            .iter()
            .filter(|(path, ..)| !seen_dirs.contains(path))
            .count();
        let mut directory_overflow = new_ancestors > dir_limit;
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
        let names = if let Some(name) = file_name {
            base.push(name);
            conn.prepare_cached(
                "SELECT id,'',logical,allocated,file_id,created_ft,reparse_tag,attrs FROM main.file WHERE id=?1 AND flags & ?2=0",
            )?
            .query_map(params![id, loomward_catalog::EXCLUDED_FLAGS], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let subdirs = conn
                .prepare_cached(&format!(
                    "{VISIBLE} SELECT v.rel,d.file_id,d.created_ft FROM visible v JOIN main.dir d ON d.id=v.id
                     WHERE v.depth>0 ORDER BY v.depth,v.id LIMIT ?3"
                ))?
                .query_map(
                    // One past the cap, so an overflow is seen and leaves the candidate unknown.
                    params![id, loomward_catalog::EXCLUDED_FLAGS, dir_limit.saturating_sub(new_ancestors) as i64 + 1],
                    |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)),
                )?
                .collect::<rusqlite::Result<Vec<(String, _, _)>>>()?;
            // Selected candidates cannot overlap, but their ancestor chains can.
            directory_overflow |= subdirs.len() > dir_limit.saturating_sub(new_ancestors);
            for (rel, file_id, created) in subdirs {
                dirs.push((base.join(rel.trim_end_matches('\\')), file_id, created));
            }
            conn.prepare_cached(&format!(
                "{VISIBLE} SELECT f.id,v.rel||f.name,f.logical,f.allocated,f.file_id,f.created_ft,f.reparse_tag,f.attrs
                 FROM main.file f JOIN visible v ON v.id=f.dir_id WHERE f.flags & ?2=0 ORDER BY f.id LIMIT ?3"
            ))?
            .query_map(
                params![id, loomward_catalog::EXCLUDED_FLAGS, name_limit as i64 + 1],
                row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(Some(Listing {
            volume_key,
            incarnation,
            subtree_rev,
            base,
            dirs,
            directory_overflow,
            names,
        }))
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
