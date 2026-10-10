//! `loomward-service`: the typed [`ViewService`] both adapters call (docs/41 section 5, LW-103).
//!
//! The service owns policy; the engine and the catalogue own work:
//! - ingress after the envelope (adapter availability, wave) and per-call deadlines
//!   (`semantics.md` sections 1 and 2): every call returns by its deadline;
//! - the 10-minute idempotency cache of mutations, replaying retained outcomes and never executing
//!   twice (errata #117 N4), and `expected_state_rev` preconditions;
//! - node IDs sealed with a per-install HMAC key over catalogue instance, row and `born_run`, and
//!   session-bound cursors (section 5, partial #15);
//! - provenance: a synthetic session accepts only fixture or identity-verified registered lab
//!   roots; a personal session needs `allow_personal` and stays teacher-disabled (ADR-V3-15);
//! - revocation commits in `state.db` first, then cancels the root's run and fences the writer, and
//!   only then emits its event (commit before event).
//!
//! No command has an effect on user files or processes (invariant 1), and no command accepts a path.

mod cursors;
mod db;
mod handlers;
mod idem;
mod ids;
pub mod lab;
pub mod paths;
#[cfg(test)]
mod tests;

use loomward_catalog::{Catalog, NodeKey, RootObservation, WriteCommand, WriteReply};
use loomward_engine::{Engine, EngineConfig};
use loomward_protocol::{
    CallContext, Command, DatasetClass, Detail, ErrorBody, ErrorCode, EventName, EventStream,
    Generation, Payload, RequestEnvelope, RequestId, ResponseEnvelope, ResponseMeta,
    RootGrantResultRefusal as Refusal, Timestamp, ViewService,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How a [`Service`] is opened.
#[derive(Debug, Clone)]
pub struct Config {
    /// Holds `state.db` and `catalog.db`; Loomward's own files only.
    pub state_dir: PathBuf,
    pub dataset: DatasetClass,
    /// A personal session is refused unless the owner passed `--allow-personal`.
    pub allow_personal: bool,
    /// Roots already checked by [`paths::validate_roots`]; consumed exactly (#146).
    pub grant_roots: Vec<PathBuf>,
}

/// The engine service. Cheap to share; `call` may run on any thread.
pub struct Service {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    dataset: DatasetClass,
    state_dir: PathBuf,
    started_at: Timestamp,
    engine: Engine,
    catalog: Catalog,
    db: Mutex<rusqlite::Connection>,
    instance: String,
    ids: ids::NodeIds,
    cursors: cursors::Cursors,
    idem: idem::Idempotency,
    /// Serialises owner-state mutations (revocation, declarations, grants).
    // ponytail: one lock for every owner-state mutation; per-resource locks if contention shows.
    mutation: Mutex<()>,
    /// Mutations run one at a time in arrival order (tickets), so a mutation whose caller timed
    /// out still lands before any later one: the owner's next choice is never overtaken by the
    /// earlier request it replaced (errata #117 N4).
    // ponytail: one global FIFO for mutations; per-resource queues if contention shows.
    turn: (Mutex<u64>, std::sync::Condvar),
    tickets: std::sync::atomic::AtomicU64,
    /// One native dialog at a time (#167): a second request is `busy`, never a stacked dialog.
    dialog: Mutex<()>,
    #[cfg(test)]
    pub(crate) hooks: tests::Hooks,
}

impl Service {
    /// Opens the stores and the engine, then grants `config.grant_roots` under the session's
    /// provenance policy. Errors name the rule, never a path.
    pub fn open(config: Config) -> Result<Service, String> {
        Self::open_with(config, idem::TTL)
    }

    pub(crate) fn open_with(config: Config, ttl: Duration) -> Result<Service, String> {
        if config.dataset == DatasetClass::Personal && !config.allow_personal {
            return Err("a personal session requires --allow-personal".into());
        }
        if paths_network(&config.state_dir) {
            return Err("the state directory must be on a local volume".into());
        }
        std::fs::create_dir_all(&config.state_dir)
            .map_err(|_| "cannot create the state directory".to_string())?;
        let state_dir = config
            .state_dir
            .canonicalize()
            .map_err(|_| "cannot resolve the state directory".to_string())?;
        let catalog = Catalog::open(&state_dir, class_name(config.dataset))
            .map_err(|e| format!("catalogue refused to open: {e}"))?;
        let engine = Engine::open(EngineConfig::new(state_dir.clone(), config.dataset))
            .map_err(|e| format!("engine refused to open: {e}"))?;
        let conn = db::open(&state_dir).map_err(|e| format!("state store: {e}"))?;
        let key = db::install_key(&conn).map_err(|e| format!("state store: {e}"))?;
        let instance = db::instance(&conn).map_err(|e| format!("state store: {e}"))?;
        let inner = Arc::new(Inner {
            dataset: config.dataset,
            state_dir,
            started_at: now(),
            engine,
            catalog,
            db: Mutex::new(conn),
            instance,
            ids: ids::NodeIds::new(key),
            cursors: cursors::Cursors::new(4096),
            idem: idem::Idempotency::new(ttl),
            mutation: Mutex::new(()),
            turn: (Mutex::new(0), std::sync::Condvar::new()),
            tickets: std::sync::atomic::AtomicU64::new(0),
            dialog: Mutex::new(()),
            #[cfg(test)]
            hooks: Default::default(),
        });
        for (i, root) in config.grant_roots.iter().enumerate() {
            inner
                .grant(root, "cli_flag")
                .map_err(|r| format!("grant root #{} refused: {}", i + 1, refusal_name(r)))?;
        }
        spawn_telemetry_pump(Arc::downgrade(&inner));
        Ok(Service { inner })
    }

    pub fn dataset(&self) -> DatasetClass {
        self.inner.dataset
    }
}

fn paths_network(p: &Path) -> bool {
    paths::check_root(p, None, &[], true) == Err(Refusal::NetworkOrRemovableUnsupported)
}

pub(crate) fn class_name(class: DatasetClass) -> &'static str {
    match class {
        DatasetClass::Synthetic => "synthetic",
        DatasetClass::Personal => "personal",
    }
}

pub(crate) fn refusal_name(r: Refusal) -> String {
    serde_json::to_value(r)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Commands that change state. Their outcome is retained under the `request_id` (section 4) and a
/// `deadline_exceeded` answer means the outcome is unknown (section 2).
pub(crate) fn is_mutation(c: Command) -> bool {
    use Command::*;
    matches!(
        c,
        RootsRequestGrant
            | RootsRevoke
            | VolumesDeclareTier
            | ScanStart
            | ScanCancel
            | FeedbackRecord
            | LearningRefit
            | CollectionsCreate
            | CollectionsUpdateMembers
            | TeacherPreview
            | GrantsCreateDisclosure
            | GrantsRevoke
            | TeacherRun
            | PlacementSimulate
            | TelemetrySubscribe
            | TelemetryUnsubscribe
            | BudgetsSet
    )
}

/// `semantics.md` section 2 defaults.
fn default_deadline(c: Command) -> u64 {
    match c {
        Command::TreeSlice => 5_000,
        Command::SearchQuery => 3_000,
        Command::PlacementSimulate => 10_000,
        c if is_mutation(c) => 5_000,
        _ => 2_000,
    }
}

/// `retryable` per `semantics.md` section 8 and #144: a timed-out mutation is never blindly
/// retryable; `internal_error` at most once, never for `teacher.run`.
pub(crate) fn retry_flag(command: Command, mut body: ErrorBody) -> ErrorBody {
    use ErrorCode::*;
    body.retryable = match body.code {
        StaleGeneration | DeviceOffline | PartialCoverage | ResourceBudget | Busy => true,
        DeadlineExceeded => !is_mutation(command),
        InternalError => command != Command::TeacherRun,
        _ => false,
    };
    body
}

pub(crate) fn fail(code: ErrorCode, message: &str) -> ErrorBody {
    ErrorBody::new(code, message, false)
}

pub(crate) fn with_reason(mut body: ErrorBody, reason: &str) -> ErrorBody {
    body.detail = Detail::new(BTreeMap::from([("reason".to_string(), json!(reason))])).ok();
    body
}

/// Catalogue errors on the wire. Messages name a rule, never a path or a row.
pub(crate) fn catalog_error(e: loomward_catalog::Error) -> ErrorBody {
    use loomward_catalog::Error as E;
    match e {
        E::Invalid("sort exceeds 10000 children") => with_reason(
            fail(
                ErrorCode::ResourceBudget,
                "this sort needs an index for directories over 10,000 children",
            ),
            "sort_requires_index",
        ),
        E::Invalid(m) => fail(ErrorCode::InvalidRequest, m),
        E::NotFound => fail(ErrorCode::NotFound, "not found"),
        E::StaleGeneration => fail(
            ErrorCode::StaleGeneration,
            "the catalogue moved; restart from the first page",
        ),
        E::RepairRequired => fail(ErrorCode::PartialCoverage, "aggregates are being repaired"),
        E::ResourceBudget => fail(ErrorCode::ResourceBudget, "catalogue budget exceeded"),
        E::Cancelled => fail(ErrorCode::Cancelled, "cancelled"),
        E::DatasetMismatch => fail(ErrorCode::PermissionDenied, "dataset_class_mismatch"),
        _ => fail(ErrorCode::InternalError, "catalogue error"),
    }
}

pub(crate) fn sql_error(_: rusqlite::Error) -> ErrorBody {
    fail(ErrorCode::InternalError, "state store error")
}

/// What a handler produced: the result object and the revisions it was read or committed at.
pub(crate) struct Out {
    pub result: Value,
    pub catalog_rev: Option<i64>,
    pub state_rev: Option<i64>,
}

impl ViewService for Service {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        let started = Instant::now();
        let id = request.request_id.clone();
        // Adapters reach here through `RequestEnvelope::from_slice`; a direct caller gets the same checks.
        let command = match request.validate() {
            Ok(c) => c,
            Err(e) => return ResponseEnvelope::error(Some(id), e, None),
        };
        if !command.available_on(ctx.adapter) {
            let e = fail(
                ErrorCode::CapabilityUnavailable,
                "this command is not offered by this adapter",
            );
            return ResponseEnvelope::error(Some(id), e, None);
        }
        if command.wave() >= 3 {
            let e = fail(
                ErrorCode::CapabilityUnavailable,
                "this command arrives with wave 3 (lane L20)",
            );
            return ResponseEnvelope::error(Some(id), e, None);
        }
        let wait = request
            .deadline_ms
            .map_or(default_deadline(command), |d| d.get() as u64);
        let deadline = started + Duration::from_millis(wait);
        if is_mutation(command) {
            self.mutate(request, command, ctx.clone(), started, deadline)
        } else {
            let inner = self.inner.clone();
            let ctx = ctx.clone();
            let (tx, rx) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("lw-read".into())
                .spawn(move || {
                    let _ = tx.send(inner.respond(&request, command, &ctx, started));
                });
            if spawned.is_err() {
                return ResponseEnvelope::error(
                    Some(id),
                    fail(ErrorCode::ResourceBudget, "no worker"),
                    None,
                );
            }
            // ponytail: a timed-out read finishes on its thread and is discarded; SQLite interrupt if load shows.
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => ResponseEnvelope::error(
                    Some(id),
                    retry_flag(
                        command,
                        fail(
                            ErrorCode::DeadlineExceeded,
                            "the read did not finish within its deadline",
                        ),
                    ),
                    None,
                ),
                Err(mpsc::RecvTimeoutError::Disconnected) => ResponseEnvelope::error(
                    Some(id),
                    retry_flag(command, fail(ErrorCode::InternalError, "the read failed")),
                    None,
                ),
            }
        }
    }

    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
        let resume = resume
            .map(|(epoch, last_seq)| loomward_engine::events::EventResume { epoch, last_seq });
        self.inner
            .engine
            .subscribe_events(resume)
            .expect("the engine event bus always opens a subscription")
    }
}

impl Service {
    /// A mutation runs at most once per `request_id`; every caller waits only until its deadline.
    fn mutate(
        &self,
        request: RequestEnvelope,
        command: Command,
        ctx: CallContext,
        started: Instant,
        deadline: Instant,
    ) -> ResponseEnvelope {
        let id = request.request_id.clone();
        let fingerprint = json!({
            "command": command.as_str(),
            "payload": request.payload,
            "expected_state_rev": request.expected_state_rev,
            "expected_generation": request.expected_generation,
        });
        match self.inner.idem.claim(&id, &fingerprint) {
            idem::Claim::Conflict => {
                let e = with_reason(
                    fail(
                        ErrorCode::InvalidRequest,
                        "this request_id was used for a different request",
                    ),
                    "idempotency_conflict",
                );
                return ResponseEnvelope::error(Some(id), e, None);
            }
            idem::Claim::Wait => {}
            idem::Claim::Run => {
                // The folder picker waits on the owner; it must never hold up a revocation.
                let ticket = (command != Command::RootsRequestGrant).then(|| {
                    self.inner
                        .tickets
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                });
                let inner = self.inner.clone();
                let key = id.to_string();
                let spawned = std::thread::Builder::new()
                    .name("lw-mutation".into())
                    .spawn(move || {
                        let mut settle = Settle {
                            inner: inner.clone(),
                            key,
                            ticket,
                            outcome: None,
                        };
                        if let Some(t) = ticket {
                            let mut serving =
                                inner.turn.0.lock().unwrap_or_else(|e| e.into_inner());
                            while *serving != t {
                                serving = inner
                                    .turn
                                    .1
                                    .wait(serving)
                                    .unwrap_or_else(|e| e.into_inner());
                            }
                        }
                        settle.outcome = Some(inner.respond(&request, command, &ctx, started));
                    });
                if spawned.is_err() {
                    // Never reached the queue: pass the turn on so later mutations are not stuck.
                    if let Some(t) = ticket {
                        Settle::advance(&self.inner, t);
                    }
                    let e = fail(ErrorCode::ResourceBudget, "no worker");
                    self.inner.idem.finish(
                        &id,
                        ResponseEnvelope::error(Some(id.clone()), e.clone(), None),
                    );
                    return ResponseEnvelope::error(Some(id), e, None);
                }
            }
        }
        self.inner.idem.wait(&id, deadline).unwrap_or_else(|| {
            ResponseEnvelope::error(
                Some(id),
                retry_flag(
                    command,
                    fail(ErrorCode::DeadlineExceeded, "outcome unknown: retry with the same request_id, or read the current state"),
                ),
                None,
            )
        })
    }
}

/// Settles a mutation: its outcome is retained (an internal error if the handler panicked, so a
/// retry never waits forever) and the next ticket gets its turn.
struct Settle {
    inner: Arc<Inner>,
    key: String,
    ticket: Option<u64>,
    outcome: Option<ResponseEnvelope>,
}

impl Settle {
    fn advance(inner: &Inner, ticket: u64) {
        let mut serving = inner.turn.0.lock().unwrap_or_else(|e| e.into_inner());
        *serving = ticket + 1;
        drop(serving);
        inner.turn.1.notify_all();
    }
}

impl Drop for Settle {
    fn drop(&mut self) {
        let outcome = self.outcome.take().unwrap_or_else(|| {
            let id = RequestId::new(&self.key).ok();
            ResponseEnvelope::error(
                id,
                fail(ErrorCode::InternalError, "the mutation failed"),
                None,
            )
        });
        self.inner.idem.finish(&self.key, outcome);
        if let Some(t) = self.ticket {
            Settle::advance(&self.inner, t);
        }
    }
}

impl Inner {
    /// Runs one command and wraps it. A result that is not exactly the command's result type is a
    /// service bug and becomes `internal_error`, never a malformed envelope.
    fn respond(
        &self,
        request: &RequestEnvelope,
        command: Command,
        ctx: &CallContext,
        started: Instant,
    ) -> ResponseEnvelope {
        let id = request.request_id.clone();
        let out = self
            .handle(request, command, ctx)
            .and_then(|out| match out.result {
                Value::Object(ref map) => command.validate_result(map).map(|_| out).map_err(|_| {
                    fail(
                        ErrorCode::InternalError,
                        "the service produced an invalid result",
                    )
                }),
                _ => Err(fail(
                    ErrorCode::InternalError,
                    "the service produced an invalid result",
                )),
            });
        match out {
            Ok(Out {
                result: Value::Object(map),
                catalog_rev,
                state_rev,
            }) => {
                let meta = self.meta(started, &map, catalog_rev, state_rev);
                ResponseEnvelope::ok_object(id, map, meta)
            }
            Ok(_) => unreachable!("validated above"),
            Err(e) => ResponseEnvelope::error(Some(id), retry_flag(command, e), None),
        }
    }

    fn meta(
        &self,
        started: Instant,
        result: &Payload,
        catalog_rev: Option<i64>,
        state_rev: Option<i64>,
    ) -> ResponseMeta {
        ResponseMeta {
            served_at: now(),
            elapsed_ms: loomward_protocol::Count::new(started.elapsed().as_millis() as u64)
                .unwrap_or(loomward_protocol::Count::ZERO),
            dataset_class: self.dataset,
            budget_hit: result
                .get("budget_hit")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            catalog_rev: catalog_rev.map(|r| Generation(r.max(0) as u64)),
            state_rev: state_rev.map(|r| Generation(r.max(0) as u64)),
        }
    }

    pub(crate) fn db(&self) -> MutexGuard<'_, rusqlite::Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn revisions(&self) -> Result<(i64, i64), ErrorBody> {
        db::revisions(&self.db()).map_err(sql_error)
    }

    /// Publishes after the commit it describes (section 6).
    pub(crate) fn publish(
        &self,
        event: EventName,
        data: Value,
        catalog_rev: Option<i64>,
        state_rev: Option<i64>,
    ) {
        if let Value::Object(map) = data {
            let _ = self.engine.publish_event(
                event,
                map,
                catalog_rev.map(|r| Generation(r as u64)),
                state_rev.map(|r| Generation(r as u64)),
            );
        }
    }

    /// Grants one already path-checked root under the session's provenance policy. A synthetic
    /// session accepts only a registered lab root whose native identity matches the registry; a
    /// personal session accepts an owner folder. Re-granting an active root returns it.
    pub(crate) fn grant(&self, root: &Path, via: &str) -> Result<(i64, bool), Refusal> {
        let (volume_key, file_id) = lab::identity(root)?;
        let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.db();
        let origin = match self.dataset {
            DatasetClass::Synthetic => {
                db::lab_root(&conn, &volume_key, &file_id)
                    .map_err(|_| Refusal::IdentityUnavailable)?
                    .ok_or(Refusal::SyntheticSessionRequiresLabRoot)?;
                "lab_generated"
            }
            DatasetClass::Personal => "owner_granted",
        };
        if let Some(grant) = db::active_grant(&conn, &volume_key, &file_id)
            .map_err(|_| Refusal::IdentityUnavailable)?
        {
            if let Some(root) =
                db::root_for_grant(&conn, grant).map_err(|_| Refusal::IdentityUnavailable)?
            {
                return Ok((root, false));
            }
        }
        drop(conn);
        let text = root.to_string_lossy();
        let display_path = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
        let display_name = root.file_name().map_or_else(
            || display_path.clone(),
            |n| n.to_string_lossy().into_owned(),
        );
        let reply = self
            .catalog
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_key,
                display_name: display_name.chars().take(260).collect(),
                display_path,
                root_file_id: Some(file_id),
                filesystem: None,
                origin: origin.into(),
                granted_via: via.into(),
                observed_at_ns: now_ns(),
            }))
            .map_err(|_| Refusal::IdentityUnavailable)?;
        match reply {
            WriteReply::Root { root_id, .. } => Ok((root_id, true)),
            _ => Err(Refusal::IdentityUnavailable),
        }
    }

    /// The row a node ID names, after its tag, catalogue instance and incarnation check out.
    /// Unknown, forged and gone IDs are `not_found` (no oracle); an ID from a rebuilt catalogue
    /// carries `catalog_instance_changed`.
    pub(crate) fn node(
        &self,
        id: &str,
        reader: &mut loomward_catalog::Reader,
    ) -> Result<NodeKey, ErrorBody> {
        let (key, born) = self.ids.open(id, &self.instance).map_err(|r| match r {
            ids::Refusal::Foreign => fail(ErrorCode::NotFound, "unknown node"),
            ids::Refusal::InstanceChanged => with_reason(
                fail(
                    ErrorCode::NotFound,
                    "the catalogue was rebuilt; refetch from the root",
                ),
                "catalog_instance_changed",
            ),
        })?;
        match key {
            NodeKey::Dir(_) | NodeKey::File(_) => reader
                .check_incarnation(key, &self.instance, born)
                .map_err(|_| fail(ErrorCode::NotFound, "unknown node"))?,
            // Virtual slice nodes: the atlas and a volume can anchor a slice (the catalogue
            // checks the volume exists); a folded "other" node names nothing.
            NodeKey::Atlas | NodeKey::Volume(_) => {}
            NodeKey::Other(_) => return Err(fail(ErrorCode::NotFound, "unknown node")),
        }
        Ok(key)
    }

    /// Replaces the catalogue's internal references with sealed node IDs, and its collection
    /// references with contract IDs, everywhere in a projection.
    pub(crate) fn seal(&self, value: &mut Value) -> Result<(), ErrorBody> {
        let mut found = Vec::new();
        walk(value, "", &mut |key, s| {
            if is_ref_key(key) {
                if let Some(k) = internal(s) {
                    found.push(k);
                }
            }
        });
        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        for k in &found {
            match k {
                NodeKey::Dir(id) => dirs.push(*id),
                NodeKey::File(id) => files.push(*id),
                _ => {}
            }
        }
        let born: std::collections::HashMap<NodeKey, i64> = {
            let conn = self.db();
            let d = db::born_runs(&conn, "dir", &dirs).map_err(sql_error)?;
            let f = db::born_runs(&conn, "file", &files).map_err(sql_error)?;
            d.into_iter()
                .map(|(id, b)| (NodeKey::Dir(id), b))
                .chain(f.into_iter().map(|(id, b)| (NodeKey::File(id), b)))
                .collect()
        };
        walk_mut(value, "", &mut |key, s| {
            if is_ref_key(key) {
                if let Some(k) = internal(s) {
                    // A row gone since the read gets an incarnation no row has: it opens as not_found.
                    let b = born.get(&k).copied().unwrap_or(
                        if matches!(k, NodeKey::Dir(_) | NodeKey::File(_)) {
                            -1
                        } else {
                            0
                        },
                    );
                    *s = self.ids.seal(k, b, &self.instance);
                }
            } else if (key == "collection_id" || key == "collection_ids") && s.starts_with("cl_") {
                *s = format!("co_{}", &s[3..]);
            }
        });
        Ok(())
    }
}

fn is_ref_key(key: &str) -> bool {
    matches!(key, "node_id" | "anchor_node_id" | "anchor")
}

/// The catalogue's internal reference spelling (`nd_dir_5`), never a wire ID.
fn internal(s: &str) -> Option<NodeKey> {
    if s == "nd_atlas" {
        return Some(NodeKey::Atlas);
    }
    let (kind, n) = s.strip_prefix("nd_")?.rsplit_once('_')?;
    let n: i64 = n.parse().ok()?;
    Some(match kind {
        "volume" => NodeKey::Volume(n),
        "dir" => NodeKey::Dir(n),
        "file" => NodeKey::File(n),
        "other" => NodeKey::Other(n),
        _ => return None,
    })
}

fn walk(v: &Value, key: &str, f: &mut dyn FnMut(&str, &str)) {
    match v {
        Value::String(s) => f(key, s),
        Value::Array(a) => a.iter().for_each(|x| walk(x, key, f)),
        Value::Object(m) => m.iter().for_each(|(k, x)| walk(x, k, f)),
        _ => {}
    }
}

fn walk_mut(v: &mut Value, key: &str, f: &mut dyn FnMut(&str, &mut String)) {
    match v {
        Value::String(s) => f(key, s),
        Value::Array(a) => a.iter_mut().for_each(|x| walk_mut(x, key, f)),
        Value::Object(m) => m.iter_mut().for_each(|(k, x)| walk_mut(x, k, f)),
        _ => {}
    }
}

/// Telemetry lease samples onto the event bus. Holds only a weak reference, so it ends with the
/// service.
fn spawn_telemetry_pump(inner: Weak<Inner>) {
    let _ = std::thread::Builder::new()
        .name("lw-telemetry-pump".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            let Some(inner) = inner.upgrade() else { return };
            for sample in inner.engine.telemetry_events().unwrap_or_default() {
                if let Ok(data) = serde_json::to_value(sample) {
                    inner.publish(EventName::TelemetrySample, data, None, None);
                }
            }
        });
}

pub(crate) fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

pub(crate) fn now() -> Timestamp {
    ts_ns(now_ns())
}

/// UTC with milliseconds and a literal `Z` (`semantics.md` section 9), from Unix nanoseconds.
pub(crate) fn ts_ns(ns: i64) -> Timestamp {
    let ms = ns.div_euclid(1_000_000).max(0);
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Timestamp::new(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    ))
    .expect("civil date is a contract timestamp")
}
