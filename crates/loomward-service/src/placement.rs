//! Catalogue-only placement and durable simulations. No content reads and no effect authority.
use crate::{db, fail, now_ns, ts_ns, with_reason, Inner, Out, ReadWork};
use loomward_engine::{placement::*, EngineError, EngineResult};
use loomward_protocol::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

fn check(deadline: Instant) -> EngineResult<()> {
    if Instant::now() >= deadline {
        Err(EngineError::DeadlineExceeded {
            message: "placement deadline expired".into(),
        })
    } else {
        Ok(())
    }
}

fn placement_error(e: EngineError) -> ErrorBody {
    let mut body = ErrorBody::from(e);
    let message = body.message.as_str().to_owned();
    if message == "personal_placement_physics_unobserved" {
        body.code = ErrorCode::CapabilityUnavailable;
        body.message = Text::new("Personal placement needs observed catalogue volume physics; this call reads catalogue metadata only.").expect("bounded refusal");
        return with_reason(body, &message);
    }
    if body.code == ErrorCode::InvalidRequest || message == "overlapping_grant_ancestry_unknown" {
        with_reason(body, &message)
    } else {
        body
    }
}
fn uint(r: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(r.get::<_, i64>(index)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(e),
        )
    })
}
fn sql(e: rusqlite::Error) -> EngineError {
    if matches!(e, rusqlite::Error::SqliteFailure(ref code, _) if code.code == rusqlite::ErrorCode::OperationInterrupted)
    {
        EngineError::DeadlineExceeded {
            message: "placement read interrupted".into(),
        }
    } else {
        EngineError::Internal {
            message: "placement store unavailable".into(),
            detail: None,
        }
    }
}

fn bounded<T>(v: Result<T, impl std::fmt::Debug>) -> EngineResult<T> {
    v.map_err(|_| EngineError::ResourceBudget {
        message: "placement projection bounds".into(),
    })
}

/// Volumes the snapshot dropped for want of an active grant are disclosed, never silently omitted.
fn with_inactive(
    mut excluded: Vec<PlacementPlanExcludedVolume>,
    conn: &Connection,
) -> EngineResult<BoundedVec<PlacementPlanExcludedVolume, 0, 64>> {
    for row in inactive_volumes(conn)? {
        if excluded
            .iter()
            .any(|v| v.volume_id.as_str() == format!("vo_{row}"))
        {
            continue;
        }
        excluded.push(PlacementPlanExcludedVolume {
            volume_id: VolumeId::new(format!("vo_{row}")).expect("volume row"),
            reason: PlacementPlanExcludedVolumeReason::NoActiveGrant,
        });
    }
    bounded(BoundedVec::new(excluded))
}

fn notes(mut plan: PlacementPlan, conn: &Connection) -> EngineResult<PlacementPlan> {
    plan.excluded_volumes = with_inactive(plan.excluded_volumes.to_vec(), conn)?;
    let mut assumptions = plan.assumptions.to_vec();
    for note in [
        "No reserve preferences are recorded; zero reserve is assumed.",
        "Catalogue-only groups have unknown relief without fresh identity-matched link observations. No file contents or per-file native metadata are read by this simulation.",
    ] {
        let note = Text::new(note).expect("bounded assumption");
        if !assumptions.contains(&note) { assumptions.push(note); }
    }
    plan.assumptions = bounded(BoundedVec::new(assumptions))?;
    Ok(plan)
}

impl Inner {
    pub(crate) fn placement_command(
        &self,
        req: &RequestEnvelope,
        command: Command,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        let deadline = work.map_or_else(|| Instant::now() + Duration::from_secs(2), |w| w.deadline);
        check(deadline)?;
        let input = Input::open(self, deadline, work)?;
        #[cfg(test)]
        if command == Command::PlacementSimulate {
            self.hooks.gate(command, "placement_read");
        }
        let store = Store {
            inner: self,
            deadline,
            conn: &input.conn,
            revisions: input.revisions,
            declaration_rev: declaration_rev(&input.conn)?,
        };
        let mut result = match command {
            Command::VolumesList => Ok(json!({"volumes":input.volumes(deadline)?.into_iter().map(|v|v.volume).collect::<Vec<_>>()})),
            Command::TiersModel => {
                let mut model = self.engine.placement_model(&input, deadline)?;
                let inactive = inactive_volumes(&input.conn)?;
                let mut volumes = model.volumes.to_vec();
                for v in &mut volumes {
                    if inactive.iter().any(|row| v.volume_id.as_str() == format!("vo_{row}")) {
                        v.source_eligible = false;
                        v.tier.note = Text::new("No active grant; excluded from placement simulation. Device speed is unmeasured.").expect("bounded note");
                    }
                }
                model.volumes = bounded(BoundedVec::new(volumes))?;
                model.policy.reserve_note = Text::new(
                    "No reserve preferences are recorded; these simulations assume zero reserve.",
                )
                .expect("bounded note");
                serde_json::to_value(model)
            }
            Command::PlacementCandidates => {
                let mut r: PlacementCandidatesRequest = req.decode_payload()?;
                r.source_volume_id =
                    VolumeId::new(format!("vo_{}", self.volume_row(&r.source_volume_id)?))
                        .expect("volume row");
                let mut candidates = self
                    .engine
                    .placement_candidates(&input, &r, deadline)
                    .map_err(placement_error)?;
                candidates.excluded_volumes =
                    with_inactive(candidates.excluded_volumes.to_vec(), &input.conn)?;
                serde_json::to_value(candidates)
            }
            Command::PlacementSimulate => {
                let mut r: PlacementSimulateRequest = req.decode_payload()?;
                r.source_volume_id =
                    VolumeId::new(format!("vo_{}", self.volume_row(&r.source_volume_id)?))
                        .expect("volume row");
                let mut overrides = r.overrides.to_vec();
                for o in &mut overrides {
                    let row = self
                        .ids
                        .open_row_id("cg_", &o.group_id, &self.instance)
                        .map_err(|_| fail(ErrorCode::InvalidRequest, "unknown group override"))?;
                    o.group_id = GroupId::new(format!("cg_{row}")).expect("group row");
                }
                r.overrides = BoundedVec::new(overrides).expect("same override bounds");
                let plan = self
                    .engine
                    .placement_simulate(
                        &input,
                        &r,
                        deadline,
                        r.save.then_some(&store as &dyn ProposalStore),
                    )
                    .map_err(placement_error)?;
                serde_json::to_value(notes(plan, &input.conn)?)
            }
            Command::ProposalsList => serde_json::to_value(self.engine.proposals_list(&store)?),
            Command::ProposalsGet => {
                let mut r: ProposalRefRequest = req.decode_payload()?;
                let row = self
                    .ids
                    .open_row_id("pp_", &r.proposal_id, &self.instance)
                    .map_err(|_| fail(ErrorCode::NotFound, "unknown proposal"))?;
                r.proposal_id = ProposalId::new(format!("pp_{row}")).expect("proposal row");
                serde_json::to_value(self.engine.proposals_get(&store, &r)?)
            }
            _ => unreachable!(),
        }
        .map_err(|_| fail(ErrorCode::InternalError, "encode placement"))?;
        self.seal(&mut result)?;
        seal_rows(self, &mut result);
        let (catalog_rev, state_rev) = input.revisions;
        let state_rev = if command == Command::PlacementSimulate
            && req.payload.get("save") == Some(&json!(true))
        {
            self.revisions()?.1
        } else {
            state_rev
        };
        Ok(Out {
            result,
            catalog_rev: Some(catalog_rev),
            state_rev: Some(state_rev),
        })
    }
}

fn seal_rows(inner: &Inner, v: &mut Value) {
    match v {
        Value::Object(m) => {
            // Planner free-space maps have volume IDs as keys, rather than values.
            if let Some(Value::Object(free)) = m.get_mut("projected_free_bytes") {
                *free = std::mem::take(free)
                    .into_iter()
                    .map(|(k, v)| {
                        let k = k
                            .strip_prefix("vo_")
                            .and_then(|s| s.parse().ok())
                            .map_or(k.clone(), |row| inner.volume_id(row));
                        (k, v)
                    })
                    .collect();
            }
            for (k, v) in m.iter_mut() {
                if let Value::String(s) = v {
                    let prefix = match k.as_str() {
                        "group_id" => Some("cg_"),
                        "proposal_id" => Some("pp_"),
                        "source_id" | "target_id" | "source_volume_id" => Some("vo_"),
                        _ => None,
                    };
                    if let Some(prefix) = prefix {
                        if let Some(row) =
                            s.strip_prefix(prefix).and_then(|s| s.parse::<i64>().ok())
                        {
                            *s = inner.ids.row_id(prefix, row, &inner.instance);
                        }
                    }
                }
                seal_rows(inner, v);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|v| seal_rows(inner, v)),
        _ => {}
    }
}

// Revocation changes permission, not online state or the historical volume observation.
fn inactive_volumes(conn: &Connection) -> EngineResult<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT v.id FROM main.volume v WHERE EXISTS(SELECT 1 FROM main.root r WHERE r.volume_id=v.id) AND NOT EXISTS(SELECT 1 FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id WHERE r.volume_id=v.id AND g.state='active') ORDER BY v.id LIMIT 65").map_err(sql)?;
    let rows = stmt
        .query_map([], |r| r.get(0))
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?;
    if rows.len() > 64 {
        return Err(EngineError::ResourceBudget {
            message: "placement volume bounds".into(),
        });
    }
    Ok(rows)
}

static SPACE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct SpacePermit;
impl Drop for SpacePermit {
    fn drop(&mut self) {
        SPACE_READS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

// The OS call cannot be cancelled; its own permit survives a caller timeout to bound stragglers.
fn bounded_space(
    deadline: Instant,
    read: impl FnOnce() -> Option<(loomward_windows::volumes::Space, i64)> + Send + 'static,
) -> EngineResult<Option<(loomward_windows::volumes::Space, i64)>> {
    check(deadline)?;
    // A compare-exchange loop: `fetch_update` is deprecated on newer stable toolchains (#276).
    let mut running = SPACE_READS.load(std::sync::atomic::Ordering::SeqCst);
    loop {
        if running >= 8 {
            return Err(EngineError::ResourceBudget {
                message: "lab volume observations still running".into(),
            });
        }
        match SPACE_READS.compare_exchange_weak(
            running,
            running + 1,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        ) {
            Ok(_) => break,
            Err(now) => running = now,
        }
    }
    let permit = SpacePermit;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("lw-volume-space".into())
        .spawn(move || {
            let _permit = permit;
            let _ = tx.send(read());
        })
        .map_err(|_| EngineError::ResourceBudget {
            message: "no volume observation worker".into(),
        })?;
    receive_space(deadline, |timeout| rx.recv_timeout(timeout))
}

fn receive_space<T>(
    deadline: Instant,
    receive: impl FnOnce(Duration) -> Result<T, std::sync::mpsc::RecvTimeoutError>,
) -> EngineResult<T> {
    // Keep an accepted receipt, including one already queued when the remaining wait is zero.
    receive(deadline.saturating_duration_since(Instant::now())).map_err(|e| match e {
        std::sync::mpsc::RecvTimeoutError::Timeout => EngineError::DeadlineExceeded {
            message: "lab volume observation deadline expired".into(),
        },
        std::sync::mpsc::RecvTimeoutError::Disconnected => EngineError::Internal {
            message: "lab volume observation worker failed".into(),
            detail: None,
        },
    })
}

/// One SQLite snapshot spans directory headers and every selected group's entries. Reads cannot
/// combine same-generation changes from separate transactions; revocation fences the next call.
pub(crate) struct Input<'a> {
    pub(crate) conn: Connection,
    inner: &'a Inner,
    pub(crate) revisions: (i64, i64),
}
impl<'a> Input<'a> {
    pub(crate) fn open(
        inner: &'a Inner,
        deadline: Instant,
        work: Option<&ReadWork>,
    ) -> EngineResult<Self> {
        check(deadline)?;
        let conn = db::open(&inner.state_dir).map_err(sql)?;
        conn.busy_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(sql)?;
        conn.progress_handler(1000, Some(move || Instant::now() >= deadline))
            .map_err(sql)?;
        if let Some(w) = work {
            *w.interrupt.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(conn.get_interrupt_handle());
        }
        conn.execute_batch("PRAGMA query_only=ON;BEGIN")
            .map_err(sql)?;
        let revisions = db::revisions(&conn).map_err(sql)?;
        Ok(Self {
            inner,
            conn,
            revisions,
        })
    }

    pub(crate) fn volumes(&self, deadline: Instant) -> EngineResult<Vec<VolumeObservation>> {
        let rows = db::volumes(&self.conn, None).map_err(sql)?;
        if rows.len() > 64 {
            return Err(EngineError::ResourceBudget {
                message: "placement volume bounds".into(),
            });
        }
        let inactive = inactive_volumes(&self.conn)?;
        let mut volumes = Vec::new();
        for v in rows {
            check(deadline)?;
            let (capabilities, cluster): (String, Option<u64>) = self
                .conn
                .query_row(
                    "SELECT capabilities_json,bytes_per_cluster FROM main.volume WHERE id=?1",
                    [v.id],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get::<_, Option<i64>>(1)?
                                .and_then(|n| u64::try_from(n).ok()),
                        ))
                    },
                )
                .map_err(sql)?;
            let stored: Value = serde_json::from_str(&capabilities).unwrap_or(Value::Null);
            let mut space = stored.get("space").cloned().unwrap_or(Value::Null);
            let mut observed_at = v.observed_at_ns;
            // Only registered synthetic roots may supply fresh volume physics. Personal placement
            // stays catalogue-only until the volume observation lane records these facts.
            if space.is_null() && self.inner.dataset == DatasetClass::Synthetic {
                let root: Option<String> = self.conn.query_row("SELECT g.display_path FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id WHERE r.volume_id=?1 AND g.state='active' AND g.origin='lab_generated' LIMIT 1", [v.id], |r| r.get(0)).optional().map_err(sql)?;
                if let Some(root) = root {
                    let key = v.key.clone();
                    if let Some((observation, at)) = bounded_space(deadline, move || {
                        let path = std::path::Path::new(&root);
                        // A replacement root cannot supply observations for the old volume.
                        if crate::lab::identity(path)
                            .ok()
                            .is_some_and(|(current, _)| current == key)
                        {
                            loomward_windows::volumes::space(path)
                                .ok()
                                .map(|space| (space, now_ns()))
                        } else {
                            None
                        }
                    })? {
                        space = serde_json::to_value(observation).expect("space serialises");
                        observed_at = at;
                    }
                }
            }
            let bytes = |key: &str| {
                space
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(Bytes)
            };
            let declared = v.declared.flatten();
            let volume = serde_json::from_value(json!({
                "volume_id":format!("vo_{}",v.id), "display_name":v.display_name.chars().take(64).collect::<String>(),
                "mount_points":[],"filesystem":v.filesystem.map(|s| s.chars().take(32).collect::<String>()),"label":null,
                "online":v.online,"read_only":space.get("read_only").and_then(Value::as_bool),"removable":null,
                "capacity_bytes":bytes("capacity_bytes"),"free_bytes":bytes("free_bytes"),
                "device":{"bus_type":"unknown","seek_penalty":null,"multiple_disks":null,"basis":"unavailable"},
                "tier":{"tier":declared,"basis":if declared.is_some(){"declared"}else{"unknown"},"declared_tier":declared,"hint_tier":null,"note":if inactive.contains(&v.id){"No active grant; excluded from placement simulation. Device speed is unmeasured."}else{"Declared tier; device speed is unmeasured."}},
                "features":{"file_ids_128":null,"hard_links":null,"sparse_files":null,"compression":null,"reparse_points":null,"usn_journal":null},
                "observed_at":ts_ns(observed_at)
            })).map_err(|_| EngineError::Internal { message: "invalid volume observation".into(), detail: None })?;
            volumes.push(VolumeObservation {
                volume,
                reserve_bytes: Bytes(0),
                cluster_bytes: space
                    .get("cluster_bytes")
                    .and_then(Value::as_u64)
                    .or(cluster),
                volume_serial: v.key.strip_prefix("vsn_").map(str::to_owned),
            });
        }
        Ok(volumes)
    }
}

struct Dir {
    id: i64,
    root: i64,
    parent: Option<i64>,
    name: String,
    file_id: Option<Vec<u8>>,
    totals: SubtreeTotals,
    newest: Option<i64>,
    unverified: bool,
}
impl PlacementInput for Input<'_> {
    fn snapshot(&self, source: Option<&VolumeId>, deadline: Instant) -> EngineResult<Snapshot> {
        check(deadline)?;
        let mut volumes = self.volumes(deadline)?;
        let Some(source) = source else {
            return Ok(Snapshot {
                volumes,
                subtrees: vec![],
                root_generations: vec![],
            });
        };
        let volume: i64 = source
            .strip_prefix("vo_")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| EngineError::InvalidRequest {
                message: "source_volume_unknown".into(),
            })?;
        let inactive = inactive_volumes(&self.conn)?;
        if inactive.contains(&volume) {
            return Err(EngineError::InvalidRequest {
                message: "no_active_grant".into(),
            });
        }
        volumes.retain(|v| {
            !inactive
                .iter()
                .any(|row| v.volume.volume_id.as_str() == format!("vo_{row}"))
        });
        if self.inner.dataset == DatasetClass::Personal
            && volumes.iter().any(|v| {
                v.volume.volume_id == *source
                    && (v.volume.capacity_bytes.is_none() || v.volume.free_bytes.is_none())
            })
        {
            return Err(EngineError::PermissionDenied {
                message: "personal_placement_physics_unobserved".into(),
            });
        }
        let roots: Vec<_> = db::roots(&self.conn, None)
            .map_err(sql)?
            .into_iter()
            .filter(|r| r.volume == Some(volume) && r.grant_state == "active")
            .collect();
        let mut stmt = self.conn.prepare("WITH RECURSIVE visible(id) AS (
            SELECT d.id FROM main.dir d JOIN main.root r ON r.id=d.root_id JOIN st.root_grant g ON g.id=r.grant_id WHERE r.volume_id=?1 AND g.state='active' AND d.parent_id IS NULL AND d.listing_state NOT IN ('excluded','absent_pending') AND d.flags & ?2=0
            UNION ALL SELECT d.id FROM main.dir d JOIN visible v ON d.parent_id=v.id WHERE d.listing_state NOT IN ('excluded','absent_pending') AND d.flags & ?2=0)
            SELECT d.id,d.root_id,d.parent_id,d.name,d.born_run,d.file_id,d.sub_files,d.sub_dirs,d.sub_logical,d.sub_allocated,d.sub_alloc_unknown,d.sub_skipped,d.sub_errors,d.sub_complete AND d.agg_valid_rev>=d.dirty_rev,d.sub_newest_ft,d.flags
            FROM main.dir d JOIN visible v ON v.id=d.id ORDER BY d.id LIMIT 10001").map_err(sql)?;
        struct RawDir {
            id: i64,
            root: i64,
            parent: Option<i64>,
            name: String,
            file_id: Option<Vec<u8>>,
            files: u64,
            sub_dirs: u64,
            logical: u64,
            allocated: u64,
            unknown: u64,
            skipped: u64,
            failed: u64,
            complete: bool,
            newest: Option<i64>,
            flags: u32,
        }
        let rows = stmt
            .query_map(params![volume, loomward_catalog::EXCLUDED_FLAGS], |r| {
                Ok(RawDir {
                    id: r.get(0)?,
                    root: r.get(1)?,
                    parent: r.get(2)?,
                    name: r.get(3)?,
                    file_id: r.get(5)?,
                    files: uint(r, 6)?,
                    sub_dirs: uint(r, 7)?,
                    logical: uint(r, 8)?,
                    allocated: uint(r, 9)?,
                    unknown: uint(r, 10)?,
                    skipped: uint(r, 11)?,
                    failed: uint(r, 12)?,
                    complete: r.get(13)?,
                    newest: r.get(14)?,
                    flags: r.get(15)?,
                })
            })
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        let budget = || EngineError::ResourceBudget {
            message: "placement directory bounds".into(),
        };
        let mut dirs = Vec::with_capacity(rows.len());
        for row in rows {
            dirs.push(Dir {
                id: row.id,
                root: row.root,
                parent: row.parent,
                name: row.name,
                file_id: row.file_id,
                totals: SubtreeTotals {
                    files: Count::new(row.files).map_err(|_| budget())?,
                    dirs: Count::new(row.sub_dirs).map_err(|_| budget())?,
                    logical_bytes: Bytes(row.logical),
                    allocated_bytes: if row.unknown == 0 {
                        Some(Bytes(row.allocated))
                    } else {
                        None
                    },
                    allocation_unknown_files: Count::new(row.unknown).map_err(|_| budget())?,
                    skipped: Count::new(row.skipped).map_err(|_| budget())?,
                    failed: Count::new(row.failed).map_err(|_| budget())?,
                    complete: row.complete,
                    stream_coverage: Default::default(),
                    unique_objects: None,
                    unique_allocated_bytes: None,
                    multi_link_entries: Count::ZERO,
                },
                newest: row.newest,
                unverified: row.flags & loomward_catalog::IDENTITY_UNVERIFIED != 0,
            });
        }
        if dirs.len() > 10000 {
            return Err(EngineError::ResourceBudget {
                message: "placement directory bounds".into(),
            });
        }
        // #219: identity-unverified rows never back a placement proposal, even a simulated one.
        if dirs.iter().any(|d| d.unverified) {
            return Err(EngineError::PermissionDenied {
                message: "identity_unverified".into(),
            });
        }
        // Collapse the same native directory seen under overlapping grants before antichain selection.
        let mut objects = BTreeMap::new();
        let mut canonical = BTreeMap::new();
        for d in &dirs {
            let id = d
                .file_id
                .as_ref()
                .map_or(d.id, |key| *objects.entry(key.clone()).or_insert(d.id));
            canonical.insert(d.id, id);
        }
        let mut parents = BTreeMap::new();
        for d in &dirs {
            if let Some(parent) = d.parent.and_then(|p| canonical.get(&p)) {
                parents.insert(canonical[&d.id], *parent);
            }
        }
        for outer in &roots {
            for inner in &roots {
                if outer.root == inner.root {
                    continue;
                }
                let prefix = format!(
                    "{}\\",
                    outer
                        .display_path
                        .trim_end_matches(['\\', '/'])
                        .to_lowercase()
                );
                if !inner
                    .display_path
                    .replace('/', "\\")
                    .to_lowercase()
                    .starts_with(&prefix)
                {
                    continue;
                }
                let outer_node = outer.dir.and_then(|id| canonical.get(&id)).copied();
                let mut node = inner.dir.and_then(|id| canonical.get(&id)).copied();
                let mut steps = 0;
                while node != outer_node && steps <= dirs.len() {
                    check(deadline)?;
                    node = node.and_then(|id| parents.get(&id)).copied();
                    if node.is_none() {
                        break;
                    }
                    steps += 1;
                }
                if node.is_none() || outer_node.is_none() || node != outer_node {
                    return Err(EngineError::PartialCoverage {
                        message: "overlapping_grant_ancestry_unknown".into(),
                    });
                }
            }
        }
        let mut subtrees = Vec::new();
        for d in &dirs {
            check(deadline)?;
            if canonical[&d.id] != d.id {
                continue;
            }
            let mut ancestors = Vec::new();
            let mut parent = parents.get(&d.id);
            while let Some(p) = parent {
                check(deadline)?;
                if *p == d.id || ancestors.len() > dirs.len() {
                    return Err(EngineError::InvalidRequest {
                        message: "cyclic_catalogue_ancestry".into(),
                    });
                }
                ancestors.push(NodeId::new(format!("nd_dir_{p}")).expect("node row"));
                parent = parents.get(p);
            }
            let newest_ns = d
                .newest
                .and_then(|ft| ft.checked_sub(116444736000000000))
                .and_then(|ft| ft.checked_mul(100));
            subtrees.push(Subtree {
                group_id: GroupId::new(format!("cg_{}", d.id)).expect("group row"),
                node_id: NodeId::new(format!("nd_dir_{}", d.id)).expect("node row"),
                root_id: RootId::new(format!("rt_{}", d.root)).expect("root row"),
                name: bounded(Text::new(d.name.clone()))?,
                ancestors,
                root_child: d
                    .parent
                    .is_some_and(|p| dirs.iter().any(|d| d.id == p && d.parent.is_none())),
                totals: d.totals.clone(),
                newest_modified_at: newest_ns.map(ts_ns),
                modified_age_days: newest_ns
                    .map(|ns| now_ns().saturating_sub(ns).max(0) as u64 / 86_400_000_000_000),
                pinned: None,
                active: None,
                protected: None,
                days_since_move: None,
            });
        }
        let root_generations = roots
            .into_iter()
            .map(|r| RootGeneration {
                root_id: RootId::new(format!("rt_{}", r.root)).expect("root row"),
                generation: r.generation.map(|g| Generation(g as u64)),
            })
            .collect();
        Ok(Snapshot {
            volumes,
            subtrees,
            root_generations,
        })
    }

    fn files(
        &self,
        subtree: &Subtree,
        _: &[RootGeneration],
        deadline: Instant,
    ) -> EngineResult<Vec<FileEntry>> {
        check(deadline)?;
        let dir: i64 = subtree
            .group_id
            .strip_prefix("cg_")
            .and_then(|s| s.parse().ok())
            .expect("internal group");
        let serial: String = self.conn.query_row("SELECT v.volume_key FROM main.dir d JOIN main.root r ON r.id=d.root_id JOIN main.volume v ON v.id=r.volume_id WHERE d.id=?1", [dir], |r|r.get(0)).map_err(sql)?;
        let mut stmt = self.conn.prepare("WITH RECURSIVE visible(id) AS (SELECT id FROM main.dir WHERE id=?1 AND listing_state NOT IN ('excluded','absent_pending') AND flags & ?2=0 UNION ALL SELECT d.id FROM main.dir d JOIN visible v ON d.parent_id=v.id WHERE d.listing_state NOT IN ('excluded','absent_pending') AND d.flags & ?2=0)
            SELECT f.id,f.logical,f.allocated,f.file_id,f.created_ft,f.reparse_tag FROM main.file f JOIN visible v ON v.id=f.dir_id WHERE f.flags & ?2=0 ORDER BY f.id LIMIT 200001").map_err(sql)?;
        let files = stmt
            .query_map(params![dir, loomward_catalog::EXCLUDED_FLAGS], |r| {
                let id: i64 = r.get(0)?;
                let file_id: Option<Vec<u8>> = r.get(3)?;
                let created: Option<i64> = r.get(4)?;
                let reparse: Option<u32> = r.get(5)?;
                let object = file_id
                    .filter(|id| id.len() == 16 || id.len() == 8)
                    .and_then(|id| {
                        serial.strip_prefix("vsn_").map(|serial| {
                            let mut bytes = [0; 16];
                            bytes[..id.len()].copy_from_slice(&id);
                            loomward_windows::identity::ObjectKey {
                                volume_serial: serial.into(),
                                file_id: bytes,
                                quality: if id.len() == 16 {
                                    loomward_windows::identity::IdentityQuality::FileId128
                                } else {
                                    loomward_windows::identity::IdentityQuality::Legacy64 {
                                        reason: "catalogue width".into(),
                                    }
                                },
                                creation_time: created.unwrap_or(0).to_string(),
                                reparse_tag: reparse.unwrap_or(0),
                            }
                        })
                    });
                Ok(FileEntry {
                    node_id: NodeId::new(format!("nd_file_{id}")).expect("file row"),
                    logical_bytes: uint(r, 1)?,
                    allocated_bytes: r
                        .get::<_, Option<i64>>(2)?
                        .and_then(|n| u64::try_from(n).ok()),
                    object,
                    link_observation: None,
                })
            })
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        check(deadline)?;
        Ok(files)
    }
}

struct Store<'a> {
    inner: &'a Inner,
    deadline: Instant,
    conn: &'a Connection,
    revisions: (i64, i64),
    declaration_rev: i64,
}
impl Store<'_> {
    fn detail(&self, conn: &Connection, row: i64) -> EngineResult<ProposalDetail> {
        check(self.deadline)?;
        let stored:Option<(String,String,String,i64)> = conn.query_row("SELECT inputs_digest,inputs_json,plan_json,created_at_ns FROM st.proposal WHERE id=?1 AND kind='placement_simulation'", [row], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(sql)?;
        let (digest, inputs, plan, at) = stored.ok_or_else(|| EngineError::NotFound {
            message: "unknown proposal".into(),
        })?;
        let inputs: Value = serde_json::from_str(&inputs).map_err(|_| EngineError::Internal {
            message: "invalid saved inputs".into(),
            detail: None,
        })?;
        let mut plan: PlacementPlan =
            serde_json::from_str(&plan).map_err(|_| EngineError::Internal {
                message: "invalid saved simulation".into(),
                detail: None,
            })?;
        let mut stale = inputs["catalogue_instance"].as_str() != Some(&self.inner.instance);
        for root in plan.root_generations.iter() {
            let row: i64 = root
                .root_id
                .strip_prefix("rt_")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| EngineError::NotFound {
                    message: "unknown proposal scope".into(),
                })?;
            let current:Option<(Option<i64>,String,String)> = conn.query_row("SELECT r.generation,r.state,g.state FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id WHERE r.id=?1", [row], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
            let Some((generation, state, grant)) = current else {
                return Err(EngineError::NotFound {
                    message: "proposal scope is unavailable".into(),
                });
            };
            if grant != "active"
                || inputs["catalogue_instance"].as_str() != Some(&self.inner.instance)
            {
                return Err(EngineError::NotFound {
                    message: "proposal scope is unavailable".into(),
                });
            }
            stale |= generation != root.generation.map(|g| g.get() as i64) || state != "complete";
        }
        stale |= inputs["catalog_rev"].as_i64() != Some(db::revisions(conn).map_err(sql)?.0)
            || inputs["declaration_rev"].as_i64() != Some(declaration_rev(conn)?);
        let id = ProposalId::new(format!("pp_{row}")).expect("proposal row");
        plan.proposal_id = Some(id.clone());
        let source: VolumeId =
            serde_json::from_value(inputs["source_volume_id"].clone()).map_err(|_| {
                EngineError::Internal {
                    message: "invalid proposal source".into(),
                    detail: None,
                }
            })?;
        Ok(ProposalDetail {
            summary: ProposalSummary {
                proposal_id: id,
                kind: Default::default(),
                status: if stale {
                    ProposalSummaryStatus::Stale
                } else {
                    ProposalSummaryStatus::SimulationOnly
                },
                created_at: ts_ns(at),
                source_volume_id: source,
                satisfied: plan.satisfied,
                shortfall_bytes: plan.shortfall_bytes,
            },
            inputs_digest: Digest::new(digest).expect("saved digest"),
            plan,
        })
    }
}
fn declaration_rev(conn: &Connection) -> EngineResult<i64> {
    conn.query_row(
        "SELECT coalesce(max(id),0) FROM st.tier_declaration",
        [],
        |r| r.get(0),
    )
    .map_err(sql)
}
impl ProposalStore for Store<'_> {
    fn save(
        &self,
        source: &VolumeId,
        digest: &Digest,
        plan: &PlacementPlan,
        deadline: Instant,
    ) -> EngineResult<PlacementPlan> {
        check(deadline)?;
        let annotated = notes(plan.clone(), self.conn)?;
        let plan = &annotated;
        let _m = self
            .inner
            .mutation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        check(deadline)?;
        let mut conn = self.inner.db();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sql)?;
        if db::revisions(&tx).map_err(sql)?.0 != self.revisions.0
            || declaration_rev(&tx)? != self.declaration_rev
        {
            return Err(EngineError::StaleGeneration {
                message: "placement inputs changed before saving".into(),
                detail: None,
            });
        }
        let digest = format!(
            "sha256:{:x}",
            Sha256::digest(
                format!(
                    "{}:{}:{}",
                    self.inner.instance,
                    digest.as_str(),
                    serde_json::to_string(&plan.excluded_volumes).expect("exclusions serialise")
                )
                .as_bytes()
            )
        );
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM st.proposal WHERE inputs_digest=?1 AND kind='placement_simulation' ORDER BY id LIMIT 1",
                [&digest],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)?;
        if let Some(row) = existing {
            let result = self.detail(&tx, row)?.plan;
            return Ok(result);
        }
        // Do not save a scenario across a revocation or catalogue change that raced its read.
        for root in plan.root_generations.iter() {
            let row: i64 = root
                .root_id
                .strip_prefix("rt_")
                .and_then(|s| s.parse().ok())
                .expect("internal root");
            let generation:Option<Option<i64>>=tx.query_row("SELECT r.generation FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id WHERE r.id=?1 AND g.state='active'", [row], |r|r.get(0)).optional().map_err(sql)?;
            if generation != Some(root.generation.map(|g| g.get() as i64)) {
                return Err(EngineError::StaleGeneration {
                    message: "proposal scope changed".into(),
                    detail: None,
                });
            }
        }
        let inputs = json!({"source_volume_id":source,"catalogue_instance":self.inner.instance,"catalog_rev":self.revisions.0,"declaration_rev":self.declaration_rev});
        tx.execute("INSERT INTO st.proposal(kind,status,inputs_digest,inputs_json,plan_json,created_at_ns) VALUES('placement_simulation','simulation_only',?1,?2,?3,?4)",params![digest,inputs.to_string(),serde_json::to_string(plan).expect("plan serialises"),now_ns()]).map_err(sql)?;
        let row = tx.last_insert_rowid();
        tx.execute(
            "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
            [],
        )
        .map_err(sql)?;
        check(deadline)?;
        tx.commit().map_err(sql)?;
        let mut plan = plan.clone();
        plan.proposal_id = Some(ProposalId::new(format!("pp_{row}")).expect("proposal row"));
        Ok(plan)
    }
    fn list(&self) -> EngineResult<ProposalList> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id FROM st.proposal WHERE kind='placement_simulation' ORDER BY id DESC",
            )
            .map_err(sql)?;
        let mut rows = stmt.query([]).map_err(sql)?;
        let mut proposals = Vec::new();
        while let Some(row) = rows.next().map_err(sql)? {
            check(self.deadline)?;
            match self.detail(self.conn, row.get(0).map_err(sql)?) {
                Ok(detail) => proposals.push(detail.summary),
                Err(EngineError::NotFound { .. }) => {}
                Err(e) => return Err(e),
            }
            if proposals.len() == 100 {
                break;
            }
        }
        Ok(ProposalList {
            proposals: bounded(BoundedVec::new(proposals))?,
        })
    }
    fn get(&self, request: &ProposalRefRequest) -> EngineResult<ProposalDetail> {
        let row = request
            .proposal_id
            .strip_prefix("pp_")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| EngineError::NotFound {
                message: "unknown proposal".into(),
            })?;
        self.detail(self.conn, row)
    }
}

#[cfg(test)]
mod followup_tests {
    use super::*;

    #[test]
    fn accepted_space_receipt_survives_post_receive_deadline_crossing() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Some((
            loomward_windows::volumes::Space {
                capacity_bytes: "1024".into(),
                free_bytes: "512".into(),
                cluster_bytes: Some(64),
                read_only: Some(false),
            },
            0,
        )))
        .unwrap();
        let deadline = Instant::now() + Duration::from_millis(50);
        let result = receive_space(deadline, |timeout| {
            let received = rx.recv_timeout(timeout);
            assert!(received.is_ok());
            assert!(
                Instant::now() < deadline,
                "fixture must receive before deadline"
            );
            std::thread::sleep(
                deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
            );
            received
        })
        .unwrap()
        .unwrap();
        assert!(Instant::now() >= deadline);
        assert_eq!(
            (
                result.0.capacity_bytes.as_str(),
                result.0.free_bytes.as_str(),
                result.1
            ),
            ("1024", "512", 0)
        );
    }

    #[test]
    fn placement_space_timeout_releases_read_permit_while_os_call_finishes() {
        let tmp = crate::tests::temp_root();
        let svc = crate::Service::open(crate::Config {
            state_dir: tmp.path().to_owned(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        let inner = svc.inner.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _permit = crate::ReadPermit::acquire(inner).unwrap();
            let result = bounded_space(Instant::now() + Duration::from_millis(100), move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                finished_tx.send(()).unwrap();
                None
            });
            drop(_permit);
            done_tx.send(result).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let result = done_rx.recv_timeout(Duration::from_secs(2));
        // Release on failure too: the mutation must fail rather than hang the test runner.
        let finished_early = finished_rx.try_recv().is_ok();
        let reads = *svc.inner.reads.lock().unwrap();
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(matches!(
            result.unwrap(),
            Err(EngineError::DeadlineExceeded { .. })
        ));
        assert!(!finished_early);
        assert_eq!(reads, 0);
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            bounded_space(Instant::now() + Duration::from_secs(2), || None)
                .unwrap()
                .is_none()
        );
    }
}
