//! Catalogue placement with live root presence and durable simulations. No content reads or effects.
use crate::{db, fail, now_ns, ts_ns, with_reason, Inner, Out, ReadWork};
use loomward_engine::{placement::*, EngineError, EngineResult};
use loomward_protocol::*;
use loomward_windows::storage_path::{
    HealthObservation, RemovalPolicy, StorageObservation, StorageQueryProvider, UsbTransport,
    WindowsStorageQueryProvider,
};
use loomward_windows::volumes::Fact;
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
        body.message = Text::new("Personal placement needs observed catalogue volume physics; this call does not refresh volume space.").expect("bounded refusal");
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
    if db::volumes(conn, None)
        .map_err(sql)?
        .iter()
        .any(|v| v.reserve_bytes.is_none())
    {
        let note = Text::new("Volumes without a recorded reserve assume zero reserve.")
            .expect("bounded assumption");
        if !assumptions.contains(&note) {
            assumptions.push(note);
        }
    }
    for note in [
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
            reserve_rev: reserve_rev(&input.conn)?,
        };
        let mut result = match command {
            Command::VolumesList => Ok(json!({"volumes":input.volumes(deadline, true, true)?.into_iter().map(|v|v.volume).collect::<Vec<_>>()})),
            Command::TiersModel => {
                let mut model = self.engine.placement_model(&input, deadline)?;
                let inactive = inactive_volumes(&input.conn)?;
                let mut volumes = model.volumes.to_vec();
                let preferences = db::volumes(&input.conn, None).map_err(sql)?;
                for v in &mut volumes {
                    v.declared_reserve_bytes = Some(preferences.iter().find(|p| v.volume_id.as_str() == format!("vo_{}", p.id)).and_then(|p| p.reserve_bytes).map(Bytes));
                    if inactive.iter().any(|row| v.volume_id.as_str() == format!("vo_{row}")) {
                        v.source_eligible = false;
                        v.tier.note = Text::new("No active grant; excluded from placement simulation. Device speed is unmeasured.").expect("bounded note");
                    }
                }
                model.volumes = bounded(BoundedVec::new(volumes))?;
                model.policy.reserve_note = Text::new(
                    "Recorded destination reserves are kept free. Unset reserves explicitly assume zero; capacity and eligibility still come from observations.",
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
type Observed = Option<(loomward_windows::volumes::Space, i64)>;
#[derive(Default)]
struct SpaceRead {
    started: Option<Instant>,
    answer: Option<Observed>,
}
/// Callers observing the same volume share one read and its start time.
type PendingRead = std::sync::Arc<(std::sync::Mutex<SpaceRead>, std::sync::Condvar)>;
/// Personal volume keys with a read running, so a stalled volume keeps one reader, not one per refresh.
static SPACE_IN_FLIGHT: std::sync::Mutex<std::collections::BTreeMap<String, PendingRead>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
/// How long a personal volume's read is awaited before that volume alone reads as unknown (#519).
const PERSONAL_SPACE_WAIT: Duration = Duration::from_millis(500);

fn space_wait_deadline(deadline: Instant, started: Option<Instant>, budget: Duration) -> Instant {
    started.map_or(deadline, |started| deadline.min(started + budget))
}

/// Releases the reader slot when the OS read finishes, not when its caller stops waiting.
struct SpacePermit {
    key: Option<String>,
}
impl Drop for SpacePermit {
    fn drop(&mut self) {
        SPACE_READS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(key) = self.key.take() {
            SPACE_IN_FLIGHT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
        }
    }
}

/// Waits, within this caller's own deadline, for the answer to a read another caller started.
fn await_pending(
    pending: &PendingRead,
    deadline: Instant,
    budget: Duration,
) -> EngineResult<Observed> {
    let (slot, ready) = &**pending;
    let mut answer = slot.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(done) = answer.answer.clone() {
            return Ok(done);
        }
        let wait_until = space_wait_deadline(deadline, answer.started, budget);
        let now = Instant::now();
        if now >= wait_until {
            return Err(EngineError::DeadlineExceeded {
                message: "lab volume observation deadline expired".into(),
            });
        }
        answer = ready
            .wait_timeout(answer, wait_until - now)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

// The OS call cannot be cancelled; its own permit survives a caller timeout to bound stragglers.
// `personal` names a volume: concurrent callers share its answer and read budget.
// Queueing is bounded by the request deadline; the personal read budget starts in the worker.
fn bounded_space(
    deadline: Instant,
    personal: Option<(&str, Duration)>,
    read: impl FnOnce() -> Observed + Send + 'static,
    #[cfg(test)] hooks: Option<std::sync::Arc<crate::tests::Hooks>>,
) -> EngineResult<Observed> {
    check(deadline)?;
    let mut in_flight = personal.map(|_| SPACE_IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner()));
    if let Some((key, budget)) = personal {
        if let Some(pending) = in_flight.as_ref().and_then(|m| m.get(key)).cloned() {
            drop(in_flight);
            return await_pending(&pending, deadline, budget);
        }
    }
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
    let mut permit = SpacePermit { key: None };
    let mut shared: Option<PendingRead> = None;
    if let Some((key, _)) = personal {
        let pending: PendingRead = Default::default();
        in_flight
            .as_mut()
            .expect("personal map locked")
            .insert(key.to_owned(), pending.clone());
        permit.key = Some(key.to_owned());
        shared = Some(pending);
    }
    drop(in_flight);
    let pending = shared.clone();
    #[cfg(test)]
    let worker_hooks = hooks.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("lw-volume-space".into())
        .spawn(move || {
            let _permit = permit;
            #[cfg(test)]
            if let Some(hooks) = &worker_hooks {
                hooks.gate(Command::VolumesList, "space_start");
            }
            if let Some(pending) = &shared {
                let (slot, ready) = &**pending;
                slot.lock().unwrap_or_else(|e| e.into_inner()).started = Some(Instant::now());
                ready.notify_all();
            }
            let answer = read();
            if let Some(pending) = shared {
                let (slot, ready) = &*pending;
                slot.lock().unwrap_or_else(|e| e.into_inner()).answer = Some(answer.clone());
                ready.notify_all();
            }
            let _ = tx.send(answer);
            #[cfg(test)]
            if let Some(hooks) = &worker_hooks {
                hooks.gate(Command::VolumesList, "space_done");
            }
        })
        .map_err(|_| EngineError::ResourceBudget {
            message: "no volume observation worker".into(),
        })?;
    if let Some((_, budget)) = personal {
        #[cfg(test)]
        if let Some(hooks) = &hooks {
            hooks.gate(Command::VolumesList, "space_wait");
        }
        await_pending(&pending.expect("personal pending read"), deadline, budget)
    } else {
        receive_space(deadline, |timeout| rx.recv_timeout(timeout))
    }
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

    /// One read-only storage-path query for an explicitly granted root; any failure is unobserved.
    /// The slow reliability-counter read gets only what the request deadline leaves (#1065), so a
    /// stalled disk degrades its own health field and never fails the listing.
    fn query_storage(
        &self,
        root: &std::path::Path,
        deadline: Instant,
    ) -> Option<StorageObservation> {
        let budget = health_budget(deadline, Instant::now());
        #[cfg(test)]
        if let Some(provider) = self
            .inner
            .hooks
            .storage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            return provider.query_within(root, budget).ok();
        }
        WindowsStorageQueryProvider.query_within(root, budget).ok()
    }

    /// `observe_personal_grants` is set only by `volumes.list` and the alerts report (#519): a
    /// personal volume holding an active owner grant gets a read-only space observation through that
    /// grant's own root. Placement and tier models share live presence but keep catalogue physics,
    /// so their personal refusal still holds. `with_storage_path` (LW-137) is set only by `volumes.list`,
    /// the one caller that reports the storage-path query; the others pay nothing for it.
    pub(crate) fn volumes(
        &self,
        deadline: Instant,
        observe_personal_grants: bool,
        with_storage_path: bool,
    ) -> EngineResult<Vec<VolumeObservation>> {
        let rows = db::volumes(&self.conn, None).map_err(sql)?;
        if rows.len() > 64 {
            return Err(EngineError::ResourceBudget {
                message: "placement volume bounds".into(),
            });
        }
        let inactive = inactive_volumes(&self.conn)?;
        let roots = db::roots(&self.conn, None).map_err(sql)?;
        let mut presence = crate::handlers::RootPresences::new();
        for root in roots.iter().filter(|r| r.grant_state == "active") {
            check(deadline)?;
            if let Some(observed) = self.inner.root_presence(root) {
                presence.insert(root.grant, observed);
            }
        }
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
            let online = crate::handlers::volume_online(&v, &roots, &presence);
            let mut classification = None;
            let mut storage = None;
            for root in roots.iter().filter(|r| {
                observe_personal_grants
                    && r.volume == Some(v.id)
                    && r.grant_state == "active"
                    && presence.get(&r.grant) == Some(&crate::handlers::RootPresence::Online)
            }) {
                check(deadline)?;
                if classification.is_none() {
                    classification = loomward_windows::volumes::classify(std::path::Path::new(
                        &root.display_path,
                    ))
                    .ok();
                }
                // LW-137: the same granted online root, one read-only query, no scan of other disks.
                if with_storage_path && storage.is_none() {
                    storage =
                        self.query_storage(std::path::Path::new(&root.display_path), deadline);
                }
            }
            let device = match &classification {
                Some(c) => {
                    json!({"bus_type": wire_bus_type(&c.bus_type), "seek_penalty": null, "multiple_disks": null, "basis": "ioctl_storage_query_property"})
                }
                None => {
                    json!({"bus_type":"unknown", "seek_penalty":null, "multiple_disks":null, "basis":"unavailable"})
                }
            };
            // Only an active grant's own root may supply fresh volume physics: a registered lab root
            // in a synthetic session, or an owner grant for `volumes.list` in a personal session.
            let observed_origin = match self.inner.dataset {
                DatasetClass::Synthetic => Some("lab_generated"),
                DatasetClass::Personal => observe_personal_grants.then_some("owner_granted"),
            };
            if let Some(origin) = observed_origin.filter(|_| space.is_null()) {
                // #553: up to four active roots on the volume, in grant order. They are tried inside the one
                // shared read of this volume, so an unobservable root falls through to the next root without
                // a second reader or a stale shared answer.
                let roots: Vec<String> = {
                    let mut stmt = self.conn.prepare("SELECT g.display_path FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id WHERE r.volume_id=?1 AND g.state='active' AND g.origin=?2 ORDER BY g.id LIMIT 4").map_err(sql)?;
                    let rows = stmt
                        .query_map(params![v.id, origin], |r| r.get(0))
                        .map_err(sql)?;
                    rows.collect::<Result<Vec<String>, _>>().map_err(sql)?
                };
                if !roots.is_empty() {
                    let key = v.key.clone();
                    let flight_key = key.clone();
                    // #530: preserve per-service test isolation while sharing across grant roots.
                    #[cfg(test)]
                    let flight_key = format!("{:p}/{flight_key}", self.inner);
                    #[cfg(test)]
                    let fake = self
                        .inner
                        .hooks
                        .space
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, fake)| *fake);
                    #[cfg(test)]
                    let hooks = self.inner.hooks.clone();
                    let read = move || {
                        #[cfg(test)]
                        if let Some(fake) = fake {
                            return fake.answer(&hooks);
                        }
                        roots.iter().find_map(|root| {
                            let path = std::path::Path::new(root);
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
                        })
                    };
                    let observed = if origin == "owner_granted" {
                        // A personal volume waits briefly and alone: an offline volume is not read, and a
                        // stall, a timeout or a busy reader leaves only that volume unknown (#519 review).
                        if online {
                            #[cfg(test)]
                            let budget = self
                                .inner
                                .hooks
                                .personal_space_wait
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .unwrap_or(PERSONAL_SPACE_WAIT);
                            #[cfg(not(test))]
                            let budget = PERSONAL_SPACE_WAIT;
                            match bounded_space(
                                deadline,
                                Some((flight_key.as_str(), budget)),
                                read,
                                #[cfg(test)]
                                None,
                            ) {
                                Ok(observed) => observed,
                                Err(
                                    EngineError::DeadlineExceeded { .. }
                                    | EngineError::ResourceBudget { .. },
                                ) => None,
                                Err(e) => return Err(e),
                            }
                        } else {
                            None
                        }
                    } else if online {
                        bounded_space(
                            deadline,
                            None,
                            read,
                            #[cfg(test)]
                            None,
                        )?
                    } else {
                        None
                    };
                    if let Some((observation, at)) = observed {
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
            let mut wire = json!({
                "volume_id":format!("vo_{}",v.id), "display_name":v.display_name.chars().take(64).collect::<String>(),
                "mount_points":[],"filesystem":v.filesystem.as_ref().map(|s| s.chars().take(32).collect::<String>()),"label":null,
                "online":online,"read_only":space.get("read_only").and_then(Value::as_bool),"removable":null,
                "capacity_bytes":bytes("capacity_bytes"),"free_bytes":bytes("free_bytes"),
                "device":device,
                "tier":{"tier":declared,"basis":if declared.is_some(){"declared"}else{"unknown"},"declared_tier":declared,"hint_tier":null,"note":if !online {crate::handlers::offline_space_note(&v, &roots, &presence)} else if inactive.contains(&v.id){"No active grant; excluded from placement simulation. Device speed is unmeasured."}else{"Declared tier; device speed is unmeasured."}},
                "features":{"file_ids_128":null,"hard_links":null,"sparse_files":null,"compression":null,"reparse_points":null,"usn_journal":null},
                "observed_at":ts_ns(observed_at)
            });
            // Absent, not null, when unobserved: the field is optional and unknown is never healthy.
            if let Some(observed) = &storage {
                wire["storage_path"] = wire_storage_path(observed);
            }
            let volume = serde_json::from_value(wire).map_err(|_| EngineError::Internal {
                message: "invalid volume observation".into(),
                detail: None,
            })?;
            volumes.push(VolumeObservation {
                volume,
                reserve_bytes: Bytes(v.reserve_bytes.unwrap_or(0)),
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
        let mut volumes = self.volumes(deadline, false, false)?;
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
                    multi_link_entries: None,
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

    /// The generations argument is unused on purpose: `snapshot()` and every `files()` and
    /// `stored_relief()` call read inside the one deferred read transaction opened in `open`
    /// (`PRAGMA query_only=ON;BEGIN`), so these rows belong to the snapshot's generations by
    /// construction and a newer generation cannot appear mid-call. A revocation or newer scan lands
    /// in a later transaction and fences the next request, not this one (#954).
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
                let object = catalogue_object(&serial, file_id, created, reparse);
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

    /// A reclaim verdict (#939) for this directory, still bound to its root's generation, active
    /// grant, node incarnation and subtree revision in this read's own snapshot.
    fn stored_relief(
        &self,
        subtree: &Subtree,
        _: &[RootGeneration],
        deadline: Instant,
    ) -> EngineResult<Option<StoredRelief>> {
        check(deadline)?;
        let dir: i64 = subtree
            .group_id
            .strip_prefix("cg_")
            .and_then(|s| s.parse().ok())
            .expect("internal group");
        let root: Option<i64> = self
            .conn
            .query_row("SELECT root_id FROM main.dir WHERE id=?1", [dir], |r| {
                r.get(0)
            })
            .optional()
            .map_err(sql)?;
        let Some(root) = root else { return Ok(None) };
        let node = loomward_catalog::NodeKey::Dir(dir);
        let Ok(stored) = self.inner.observed_relief(&self.conn, root, Some(node)) else {
            // An unreadable verdict leaves this group's relief unknown, as before #939; a deadline still surfaces.
            check(deadline)?;
            return Ok(None);
        };
        check(deadline)?;
        Ok(stored.first().map(|&(_, bytes, at)| StoredRelief {
            bytes,
            observed_at: ts_ns(at),
        }))
    }
}

/// A catalogue file row's listed identity, as the link-count pass expects it (`linkcount::Entry`).
pub(crate) fn catalogue_object(
    volume_key: &str,
    file_id: Option<Vec<u8>>,
    created: Option<i64>,
    reparse: Option<u32>,
) -> Option<loomward_windows::identity::ObjectKey> {
    let id = file_id.filter(|id| id.len() == 16 || id.len() == 8)?;
    let serial = volume_key.strip_prefix("vsn_")?;
    let mut bytes = [0; 16];
    bytes[..id.len()].copy_from_slice(&id);
    Some(loomward_windows::identity::ObjectKey {
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
    })
}

struct Store<'a> {
    inner: &'a Inner,
    deadline: Instant,
    conn: &'a Connection,
    revisions: (i64, i64),
    declaration_rev: i64,
    reserve_rev: i64,
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
            || inputs["declaration_rev"].as_i64() != Some(declaration_rev(conn)?)
            || inputs["reserve_rev"].as_i64().unwrap_or(0) != reserve_rev(conn)?;
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
fn reserve_rev(conn: &Connection) -> EngineResult<i64> {
    conn.query_row(
        "SELECT coalesce(max(id),0) FROM st.reserve_declaration",
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
            || reserve_rev(&tx)? != self.reserve_rev
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
                    "{}:{}:{}:{}",
                    self.inner.instance,
                    digest.as_str(),
                    serde_json::to_string(&plan.excluded_volumes).expect("exclusions serialise"),
                    self.reserve_rev
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
        let inputs = json!({"source_volume_id":source,"catalogue_instance":self.inner.instance,"catalog_rev":self.revisions.0,"declaration_rev":self.declaration_rev,"reserve_rev":self.reserve_rev});
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

/// Time kept back for the rest of the response after a health read.
const HEALTH_RESERVE: Duration = Duration::from_millis(250);

/// What the request deadline leaves for one health read; zero skips it.
pub(crate) fn health_budget(deadline: Instant, now: Instant) -> Duration {
    deadline
        .saturating_duration_since(now)
        .saturating_sub(HEALTH_RESERVE)
}

/// LW-137: the wire form of one storage-path observation. An unknown fact is `null`; reported
/// counters are evidence and there is no healthy state, so a hidden or empty result stays unknown.
pub(crate) fn wire_storage_path(observed: &StorageObservation) -> Value {
    fn known<T: Clone>(fact: &Fact<T>) -> Option<T> {
        match fact {
            Fact::Known { value } => Some(value.clone()),
            Fact::Unknown { .. } => None,
        }
    }
    fn clip(text: &str, max: usize) -> String {
        text.chars().take(max).collect()
    }
    let count = |n: Option<u64>| n.map(|n| n.min(9_007_199_254_740_991));
    let path = &observed.storage_path;
    let health = match &observed.health {
        HealthObservation::CountersPresent { counters: c } => json!({
            "state": "counters_present", "reason": null,
            "counters": {
                "temperature_celsius": count(c.temperature_celsius.map(u64::from)),
                "read_errors_total": count(c.read_errors_total),
                "read_errors_uncorrected": count(c.read_errors_uncorrected),
                "write_errors_total": count(c.write_errors_total),
                "write_errors_uncorrected": count(c.write_errors_uncorrected),
                "power_on_hours": count(c.power_on_hours),
                "wear_percent": count(c.wear_percent.map(u64::from)),
                "start_stop_cycles": count(c.start_stop_cycles),
                "critical_warning": c.critical_warning.map(|w| json!({
                    "available_spare_low": w.available_spare_low,
                    "temperature": w.temperature,
                    "reliability_degraded": w.reliability_degraded,
                    "read_only": w.read_only,
                    "volatile_backup_failed": w.volatile_backup_failed,
                    "other_bits": w.other_bits,
                })),
                "available_spare_percent": count(c.available_spare_percent.map(u64::from)),
                "available_spare_threshold_percent": count(c.available_spare_threshold_percent.map(u64::from)),
                "media_errors": count(c.media_errors),
                "unsafe_shutdowns": count(c.unsafe_shutdowns),
            },
        }),
        HealthObservation::UnsupportedOrHidden { reason } => json!({
            "state": "unsupported_or_hidden", "reason": clip(reason, 256), "counters": null,
        }),
    };
    json!({
        "bus_type": known(&path.bus_type).filter(|b| b != "unknown").map(|b| wire_bus_type(&b).to_owned()),
        "bridge": known(&path.bridge).map(|b| clip(&b, 64)),
        "usb_transport": known(&path.usb_transport).map(|t| match t {
            UsbTransport::Bot => "bot",
            UsbTransport::Uas => "uas",
        }),
        "logical_sector_bytes": known(&path.logical_sector_bytes),
        "physical_sector_bytes": known(&path.physical_sector_bytes),
        "removal_policy": known(&path.removal_policy).map(|p| match p {
            RemovalPolicy::ExpectNoRemoval => "expect_no_removal",
            RemovalPolicy::ExpectOrderlyRemoval => "expect_orderly_removal",
            RemovalPolicy::ExpectSurpriseRemoval => "expect_surprise_removal",
        }),
        "write_cache_enabled": known(&path.write_cache_enabled),
        "health": health,
    })
}

/// The protocol's bus names; any other native bus is `other` (#888 item 6 keeps tests on this map).
pub(crate) fn wire_bus_type(native: &str) -> &str {
    match native {
        "nvme" | "sata" | "sas" | "scsi" | "usb" | "raid" | "virtual" | "sd" => native,
        _ => "other",
    }
}

#[cfg(test)]
mod followup_tests {
    use super::*;

    #[test]
    fn personal_space_budget_starts_at_the_read_not_while_worker_is_queued() {
        let queued = Instant::now();
        let deadline = queued + Duration::from_secs(60);
        // A worker held before the read starts still has its full read budget.
        assert_eq!(
            space_wait_deadline(deadline, None, PERSONAL_SPACE_WAIT),
            deadline
        );
        let started = queued + Duration::from_secs(1);
        assert_eq!(
            space_wait_deadline(deadline, Some(started), PERSONAL_SPACE_WAIT),
            started + Duration::from_millis(500)
        );
        assert_eq!(
            space_wait_deadline(started, Some(started), PERSONAL_SPACE_WAIT),
            started
        );
    }

    #[test]
    fn a_queued_space_worker_keeps_its_read_budget_and_accepted_receipt() {
        use crate::tests::Hooks;
        use std::sync::Arc;
        let hooks = Arc::new(Hooks::default());
        // Zero makes any charge for spawn/scheduling observable; gates replace elapsed-time races.
        *hooks.personal_space_wait.lock().unwrap() = Some(Duration::ZERO);
        for at in ["space_start", "space_wait", "space_done"] {
            hooks
                .closed
                .lock()
                .unwrap()
                .push((Command::VolumesList, at));
        }
        struct Release(Arc<Hooks>);
        impl Drop for Release {
            fn drop(&mut self) {
                for at in ["space_start", "space_wait", "space_done"] {
                    self.0.open(Command::VolumesList, at);
                }
            }
        }
        let _release = Release(hooks.clone());
        let caller_hooks = hooks.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let caller = std::thread::spawn(move || {
            let budget = caller_hooks.personal_space_wait.lock().unwrap().unwrap();
            let result = bounded_space(
                Instant::now() + Duration::from_secs(60),
                Some(("queued-space-regression/root", budget)),
                || {
                    Some((
                        loomward_windows::volumes::Space {
                            capacity_bytes: "1024".into(),
                            free_bytes: "512".into(),
                            cluster_bytes: None,
                            read_only: Some(false),
                        },
                        0,
                    ))
                },
                Some(caller_hooks.clone()),
            );
            tx.send(result).unwrap();
            caller_hooks.gate(Command::VolumesList, "space_call_done");
        });
        hooks.until("queued caller waits or returns", |h| {
            h.arrived.lock().unwrap().iter().any(|g| {
                *g == (Command::VolumesList, "space_wait")
                    || *g == (Command::VolumesList, "space_call_done")
            })
        });
        let waiting = hooks
            .arrived
            .lock()
            .unwrap()
            .contains(&(Command::VolumesList, "space_wait"));
        if waiting {
            hooks.open(Command::VolumesList, "space_start");
            hooks.until("space receipt queued", |h| {
                h.arrived
                    .lock()
                    .unwrap()
                    .contains(&(Command::VolumesList, "space_done"))
            });
            hooks.open(Command::VolumesList, "space_wait");
        }
        let result = rx.recv_timeout(Duration::from_secs(60)).unwrap();
        drop(_release);
        caller.join().unwrap();
        let (space, _) = result
            .expect("queueing must not expire the read budget")
            .unwrap();
        assert!(waiting, "the worker must be allowed to start");
        assert_eq!(space.capacity_bytes, "1024");
        assert_eq!(space.free_bytes, "512");
    }

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
            let result = bounded_space(
                Instant::now() + Duration::from_millis(100),
                None,
                move || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    finished_tx.send(()).unwrap();
                    None
                },
                None,
            );
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
            bounded_space(Instant::now() + Duration::from_secs(2), None, || None, None)
                .unwrap()
                .is_none()
        );
    }
}
