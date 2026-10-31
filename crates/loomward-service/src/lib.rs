//! `loomward-service`: the typed [`ViewService`] both adapters call (docs/41 section 5, LW-103).
//!
//! The service owns policy; the engine and the catalogue own work:
//! - ingress after the envelope (adapter availability, wave) and per-call deadlines
//!   (`semantics.md` sections 1 and 2): every call returns by its deadline;
//! - the 10-minute idempotency cache of mutations, replaying retained outcomes and never executing
//!   twice (errata #117 N4), and `expected_state_rev` preconditions;
//! - node IDs sealed with a per-session HMAC key over catalogue instance, row and `born_run`, and
//!   session-bound cursors (section 5, partial #15);
//! - provenance: a synthetic session accepts only fixture or identity-verified registered lab
//!   roots; a personal session needs `allow_personal` and stays teacher-disabled (ADR-V3-15);
//! - revocation commits in `state.db` first, then cancels the root's run and fences the writer, and
//!   only then emits its event (commit before event).
//!
//! No command has an effect on user files or processes (invariant 1), and no command accepts a path.
#![forbid(unsafe_code)]

mod alerts;
mod apps;
mod ask;
mod backups;
mod changes;
mod client_commands;
mod client_refs;
mod clients;
mod clutter;
mod collections;
mod copy;
mod covers;
mod cursors;
mod db;
mod desktop;
mod diagnostics;
mod duplicates;
mod feedback_history;
mod freshness;
mod handlers;
#[cfg(feature = "test-support")]
pub mod hostile;
mod idem;
mod ids;
mod insight;
mod inventory;
pub mod lab;
mod learning;
mod meaning;
pub mod paths;
mod placement;
mod projects;
mod reclaim;
mod sink;
#[cfg(test)]
mod tests;

pub use client_refs::{ClientReference, ClientReferenceIds};
pub use clients::CLIENT_FIELDS;
use loomward_catalog::{
    Catalog, CatalogOptions, NodeKey, RootObservation, WriteCommand, WriteReply,
};

/// `features.federated_search`, and with it the catalogue's ranked token index (#520): the lane
/// that serves ranked search flips this one constant, so the open-time backfill never runs for a
/// store the owner has not opted into. docs/46 F1 is the contract only.
pub(crate) const FEDERATED_SEARCH: bool = false;
use loomward_engine::{Engine, EngineConfig};
use loomward_protocol::{
    CallContext, Command, DatasetClass, Detail, ErrorBody, ErrorCode, EventName, EventStream,
    Generation, Payload, Principal, RequestEnvelope, RequestId, ResponseEnvelope, ResponseMeta,
    RootGrantResultRefusal as Refusal, Timestamp, ViewService,
};
use rusqlite::OptionalExtension;
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
    /// #219: the owner's explicit `--allow-identity-unverified`: a personal `grant_roots` entry on
    /// a FAT, exFAT or unknown filesystem is granted identity-unverified instead of refused.
    pub allow_identity_unverified: bool,
}

/// The engine service. Cheap to share; `call` may run on any thread.
pub struct Service {
    inner: OwnedInner,
    /// A handle an Ask turn borrows for its tool calls (ask.rs): dropping it shuts nothing down.
    peer: bool,
}

/// An in-process client's reference lifetime witness. Never sent to a model or persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientViewStamp {
    epoch: i64,
    catalog_rev: i64,
    roots: Vec<i64>,
    instance: String,
}

/// Shared borrows stay unchanged; shutdown can take and atomically reclaim the owning Arc.
struct OwnedInner(Option<Arc<Inner>>);

impl std::ops::Deref for OwnedInner {
    type Target = Arc<Inner>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("service is not being dropped")
    }
}

pub(crate) struct Inner {
    weak: Weak<Inner>,
    freshness: freshness::Tracker,
    dataset: DatasetClass,
    state_dir: PathBuf,
    started_at: Timestamp,
    engine: Arc<Engine>,
    catalog: Catalog,
    /// The catalogue-backed scan writer installed into the engine.
    scan: Arc<sink::CatalogScanSink>,
    db: Mutex<rusqlite::Connection>,
    instance: String,
    ids: ids::NodeIds,
    client_open: Option<(client_refs::OpenWitness, client_refs::OpenWitness)>,
    cursors: cursors::Cursors,
    idem: idem::Idempotency,
    /// Pending root/volume replies in adapters; no root identifiers or generation history.
    reply_delivery: loomward_protocol::ResponseDeliveryPolicy,
    /// Human history also withholds pending names across revocation.
    history_delivery: loomward_protocol::ResponseDeliveryPolicy,
    /// Serialises owner-state mutations (revocation, declarations, grants).
    // ponytail: one lock for every owner-state mutation; per-resource locks if contention shows.
    mutation: Mutex<()>,
    /// Mutations run one at a time in arrival order (tickets), so a mutation whose caller timed
    /// out still lands before any later one: the owner's next choice is never overtaken by the
    /// earlier request it replaced (errata #117 N4). At most [`MAX_PENDING`] tickets are
    /// outstanding; revocations and the picker take none.
    // ponytail: one global FIFO for mutations; per-resource queues if contention shows.
    turn: (Mutex<Queue>, std::sync::Condvar),
    /// One native dialog at a time (#167): a second request is `busy`, never a stacked dialog.
    dialog: Mutex<()>,
    picker_last: Mutex<Option<Instant>>,
    /// The last diagnostic preview's digest and exact bytes (one slot, ADR-V3-24).
    preview: Mutex<diagnostics::Held>,
    /// docs/47 C5: copy-and-verify for synthetic sessions on lab roots (copy.rs).
    pub(crate) copies: copy::Copies,
    reads: Mutex<usize>,
    metadata: Arc<dyn inventory::Provider>,
    metadata_cursors: cursors::Cursors,
    /// LW-118: the latest Ask turn and the provider factory.
    pub(crate) ask: ask::Asks,
    /// #1106: the newest finished `coverage.result` reports (covers.rs).
    covers: covers::Results,
    /// Three-second display hints only; native admission never trusts this cache.
    root_presence_cache: Mutex<handlers::RootPresenceCache>,
    #[cfg(test)]
    pub(crate) hooks: Arc<tests::Hooks>,
}

/// Waits up to ten seconds for detached workers' shared state, then reclaims it atomically so
/// weak upgrades cannot outlive successful reclamation. Workers past that deadline, or scans
/// retaining a separate sink, may still hold catalogue connections after this returns.
/// Destruction of the reclaimed state also runs the engine and copy workers' own shutdowns.
impl Drop for Service {
    fn drop(&mut self) {
        if self.peer {
            return;
        }
        let mut inner = self.inner.0.take().expect("service owns its state");
        inner.freshness.stop();
        let give_up = Instant::now() + Duration::from_secs(10);
        #[cfg(test)]
        inner.hooks.gate(Command::TelemetrySubscribe, "drop_ready");
        loop {
            match Arc::try_unwrap(inner) {
                Ok(inner) => {
                    #[cfg(test)]
                    if let Some(after_check) = inner.hooks.after_drop_check.lock().unwrap().take() {
                        after_check();
                    }
                    drop(inner);
                    return;
                }
                Err(shared) => inner = shared,
            }
            if Instant::now() >= give_up {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Service {
    /// Opens the stores and the engine, then grants `config.grant_roots` under the session's
    /// provenance policy. Errors name the rule, never a path.
    pub fn open(config: Config) -> Result<Service, String> {
        Self::open_with(config, idem::TTL, true)
    }

    /// Records explicit owner grants for one-shot verification without watchers, automatic scans,
    /// telemetry, copy recovery or backup cleanup. Store initialization/migrations still run.
    pub fn open_for_verification(config: Config) -> Result<Service, String> {
        Self::open_stores(config, idem::TTL, false, None, None, true)
    }

    /// Opens client views without watchers, automatic scans, telemetry or backup cleanup.
    /// Store initialization/migrations still run. The adapter must use `CallContext::client`.
    pub fn open_client(config: Config) -> Result<Service, String> {
        if !config.grant_roots.is_empty() {
            return Err("a client host cannot grant roots".into());
        }
        Self::open_stores(config, idem::TTL, false, None, None, true)
    }

    /// Recheck before and after resolving an adapter's in-memory reference, alongside the normal
    /// client call. A re-grant or catalogue change invalidates the adapter's previous references.
    pub fn client_view_stamp(&self, client: &str) -> Result<ClientViewStamp, ErrorBody> {
        let grant = self.inner.client_grant(client)?;
        Ok(ClientViewStamp {
            epoch: grant.epoch,
            catalog_rev: self.inner.revisions()?.0,
            roots: grant.roots,
            instance: self.inner.instance.clone(),
        })
    }

    /// Refuses static overlap with stored and proposed grants before recording any new grant.
    pub fn open_with_static(config: Config, static_dir: Option<&Path>) -> Result<Service, String> {
        Self::open_stores(config, idem::TTL, true, static_dir, None, false)
    }

    /// `scan_writer: false` leaves the engine without a catalogue scan writer (tests that install
    /// their own probe).
    pub(crate) fn open_with(
        config: Config,
        ttl: Duration,
        scan_writer: bool,
    ) -> Result<Service, String> {
        Self::open_stores(config, ttl, scan_writer, None, None, false)
    }

    fn open_stores(
        config: Config,
        ttl: Duration,
        scan_writer: bool,
        static_dir: Option<&Path>,
        metadata: Option<Arc<dyn inventory::Provider>>,
        client_only: bool,
    ) -> Result<Service, String> {
        if config.dataset == DatasetClass::Personal && !config.allow_personal {
            return Err("a personal session requires --allow-personal".into());
        }
        let destination = paths::prospective_dir(&config.state_dir)?;
        let sqlite_destination = loomward_catalog::sqlite_dir(&destination);
        // Grant the canonical spellings the rules approved, never the caller's raw text (#710).
        let roots = paths::validate_roots(&config.grant_roots, Some(&destination))?;
        let stores = Catalog::validate_stores(&destination, class_name(config.dataset))
            .map_err(|e| format!("state store: {e}"))?;
        let mut protected = roots.clone();
        for root in &roots {
            let (volume, file_id) = lab::identity(root).map_err(refusal_name)?;
            if config.dataset == DatasetClass::Synthetic {
                if file_id.len() != 16 {
                    return Err(refusal_name(Refusal::IdentityUnavailable));
                }
                let existing = rusqlite::Connection::open_with_flags(
                    sqlite_destination.join("state.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .map_err(|_| refusal_name(Refusal::SyntheticSessionRequiresLabRoot))?;
                // Preflight is read-only: an unmigrated registration is not provenance.
                if !existing.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('lab_root') WHERE name='volume_guid')", [], |r| r.get::<_, bool>(0))
                    .map_err(|_| refusal_name(Refusal::IdentityUnavailable))? {
                    return Err(refusal_name(Refusal::SyntheticSessionRequiresLabRoot));
                }
                let volume_guid = lab::volume_guid(root).map_err(refusal_name)?;
                let digest: Option<String> = existing.query_row("SELECT manifest_digest FROM lab_root WHERE volume_key=?1 AND root_file_id=?2 AND volume_guid=?3", rusqlite::params![volume, file_id, volume_guid], |r| r.get(0))
                    .optional().map_err(|_| refusal_name(Refusal::IdentityUnavailable))?;
                if !digest.as_deref().is_some_and(lab::valid_digest) {
                    return Err(refusal_name(Refusal::SyntheticSessionRequiresLabRoot));
                }
            }
        }
        for name in ["state.db", "catalog.db"] {
            if !destination.join(name).exists() || (name == "catalog.db" && static_dir.is_none()) {
                continue;
            }
            let existing = rusqlite::Connection::open_with_flags(
                sqlite_destination.join(name),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|_| "cannot inspect stored grants")?;
            // V1 grants migrate from the catalogue; a failed version read must refuse startup.
            if name == "catalog.db" && !stored_grant_catalogue_is_legacy(&existing)? {
                continue;
            }
            // Empty and pre-v2 state stores have no grant table yet; keep preflight read-only.
            if !existing
                .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='root_grant')", [], |r| r.get::<_, bool>(0))
                .map_err(|_| "cannot inspect stored grants")?
            {
                continue;
            }
            let mut stmt = existing
                .prepare("SELECT display_path FROM root_grant WHERE state='active'")
                .map_err(|_| "cannot inspect stored grants")?;
            protected.extend(
                stmt.query_map([], |r| r.get::<_, String>(0).map(PathBuf::from))
                    .map_err(|_| "cannot inspect stored grants")?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|_| "cannot inspect stored grants")?,
            );
        }
        paths::validate_static_dir(static_dir, &protected, Some(&destination))?;
        std::fs::create_dir_all(&config.state_dir)
            .map_err(|_| "cannot create the state directory".to_string())?;
        let state_dir = config
            .state_dir
            .canonicalize()
            .map_err(|_| "cannot resolve the state directory".to_string())?;
        let client_before = client_only
            .then(|| client_refs::OpenWitness::read(&sqlite_destination))
            .flatten();
        // The ranked token index exists only where ranked search is served (#520).
        let catalog = stores
            .open(&CatalogOptions {
                ranked_index: FEDERATED_SEARCH,
            })
            .map_err(|e| format!("catalogue refused to open: {e}"))?;
        // A root's first full scan publishes once, atomically, with its file indexes built once (#148).
        catalog.writer().set_bulk_first_scan(true);
        let engine = Engine::open(EngineConfig::new(state_dir.clone(), config.dataset))
            .map_err(|e| format!("engine refused to open: {e}"))?;
        let mut conn = db::open(&state_dir).map_err(|e| format!("state store: {e}"))?;
        // Once per open, not per connection: it takes the write lock (#607).
        clients::migrate(&mut conn).map_err(|e| format!("client grants: {e}"))?;
        let forgotten: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM st.root_forgotten)", [], |r| {
                r.get(0)
            })
            .map_err(|e| format!("forget recovery: {e}"))?;
        engine.install_learning(
            loomward_engine::learning::Store::open(&state_dir)
                .map_err(|e| format!("learning store: {e}"))?,
        );
        if static_dir.is_some() {
            let mut protected = roots.clone();
            protected
                .extend(db::active_root_paths(&conn).map_err(|e| format!("state store: {e}"))?);
            paths::validate_static_dir(static_dir, &protected, Some(&state_dir))?;
        }
        let instance = db::instance(&conn).map_err(|e| format!("state store: {e}"))?;
        let lookups = db::open(&state_dir).map_err(|e| format!("state store: {e}"))?;
        let client_after = client_only
            .then(|| client_refs::OpenWitness::read(&sqlite_destination))
            .flatten();
        let client_open = client_before.zip(client_after);
        let inner = Arc::new_cyclic(|weak| Inner {
            weak: weak.clone(),
            freshness: Default::default(),
            scan: Arc::new(sink::CatalogScanSink::new(weak.clone(), lookups)),
            dataset: config.dataset,
            state_dir,
            started_at: now(),
            engine: Arc::new(engine),
            catalog,
            db: Mutex::new(conn),
            instance,
            ids: ids::NodeIds::session(),
            client_open,
            cursors: cursors::Cursors::new(4096),
            idem: idem::Idempotency::new(ttl),
            reply_delivery: loomward_protocol::ResponseDeliveryPolicy::default(),
            history_delivery: loomward_protocol::ResponseDeliveryPolicy::default(),
            mutation: Mutex::new(()),
            turn: (Mutex::new(Queue::default()), std::sync::Condvar::new()),
            dialog: Mutex::new(()),
            picker_last: Mutex::new(None),
            preview: Mutex::new(None),
            copies: copy::Copies::open(&config.state_dir, config.dataset, !client_only),
            reads: Mutex::new(0),
            metadata: metadata
                .clone()
                .unwrap_or_else(|| inventory::provider(config.dataset)),
            metadata_cursors: cursors::Cursors::new(16),
            ask: Default::default(),
            covers: Default::default(),
            root_presence_cache: Default::default(),
            #[cfg(test)]
            hooks: Default::default(),
        });
        if forgotten && !client_only {
            let _ = inner.scrub_backups(None); // Pending cleanup is exposed by health.get, not fatal.
        }
        if scan_writer {
            inner
                .engine
                .set_scan_sink(inner.scan.clone())
                .map_err(|e| format!("engine refused the scan writer: {e}"))?;
        }
        for (i, root) in roots.iter().enumerate() {
            inner
                .grant(root, "cli_flag", config.allow_identity_unverified)
                .map_err(|r| grant_refusal(i + 1, &r, !client_only))?;
        }
        if scan_writer {
            freshness::start(&inner);
        }
        if !client_only {
            spawn_telemetry_pump(Arc::downgrade(&inner));
            spawn_backup_scrub_retry(Arc::downgrade(&inner));
        }
        Ok(Service {
            inner: OwnedInner(Some(inner)),
            peer: false,
        })
    }

    pub fn dataset(&self) -> DatasetClass {
        self.inner.dataset
    }
}

/// `flag_offered`: the host has `--allow-identity-unverified`. Client-only hosts, including the
/// one-shot verification open, have none, so the text must not send their users to it (#903).
pub(crate) fn grant_refusal(number: usize, refused: &Refused, flag_offered: bool) -> String {
    let rule = match refused {
        Refused::Rule(r) => refusal_name(*r),
        Refused::RootLimit => "root_limit_reached".into(),
        Refused::IdentityUnverified if flag_offered => {
            "identity_unverified (pass --allow-identity-unverified to list it read-only)".into()
        }
        Refused::IdentityUnverified => "identity_unverified (a FAT, exFAT or unknown filesystem cannot prove file identity, so this command reads no content there; no option or state directory changes that)"
            .into(),
    };
    format!("grant root #{number} refused: {rule}")
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

/// Why a grant was refused: a contract refusal rule, or the active-root cap (the contract lists
/// at most 64 roots, so a 65th active root could not be shown or revoked).
#[derive(Debug, PartialEq)]
pub(crate) enum Refused {
    Rule(Refusal),
    RootLimit,
    /// #219: a personal root on a FAT, exFAT or unknown filesystem the owner has not confirmed.
    IdentityUnverified,
}

impl From<Refusal> for Refused {
    fn from(r: Refusal) -> Self {
        Refused::Rule(r)
    }
}

/// Commands that change state. Their outcome is retained under the `request_id` (section 4) and a
/// `deadline_exceeded` answer means the outcome is unknown (section 2).
pub(crate) fn is_mutation(c: Command) -> bool {
    use Command::*;
    matches!(
        c,
        RootsRequestGrant
            | RootsRevoke
            | RootsForget
            | VolumesDeclareTier
            | ScanStart
            | ScanCancel
            | JobsCancel
            | DuplicatesInspect
            | DuplicatesCancel
            | CoverageStart
            | ReclaimObserve
            | ReclaimSample
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
            | BackupsImport
            | BackupsForget
            | DiagnosticsSave
            | AlertsConfigure
            // docs/47: refused before dispatch while the gate is closed; mutations when it opens.
            | CopyApprove
            | CopyStart
            | CopyCancel
            | CopyAbandon
            | AskConfigure
            | AskGrant
            | ClientsGrant
            | ClientsRevoke
            | AskRevoke
            | AskTurn
    )
}

/// Ticketed mutations issued but not yet settled; one more is `busy` before any work runs.
pub(crate) const MAX_PENDING: u64 = 64;

const MAX_READS: usize = 8;

fn guarded_observation(request: &RequestEnvelope, command: Command) -> bool {
    matches!(command, Command::FeedbackHistory | Command::AlertsHistory)
        || request.expected_generation.is_some()
            && matches!(
                command,
                Command::NodeInspect | Command::StatsBreakdown | Command::TreeChildren
            )
}

struct ReadPermit(Arc<Inner>);
impl ReadPermit {
    fn acquire(inner: Arc<Inner>) -> Option<Self> {
        let mut count = inner.reads.lock().unwrap_or_else(|e| e.into_inner());
        if *count >= MAX_READS {
            return None;
        }
        *count += 1;
        drop(count);
        Some(Self(inner))
    }
}
impl Drop for ReadPermit {
    fn drop(&mut self) {
        *self.0.reads.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
    }
}

pub(crate) struct ReadWork {
    deadline: Instant,
    interrupt: Mutex<Option<rusqlite::InterruptHandle>>,
    repair_on_read: bool,
}
impl ReadWork {
    fn reader(&self, catalog: &Catalog) -> Result<loomward_catalog::Reader, ErrorBody> {
        let mut reader = catalog.reader().map_err(catalog_error)?;
        if !self.repair_on_read {
            reader = reader.without_repairs();
        }
        let mut handle = self.interrupt.lock().unwrap_or_else(|e| e.into_inner());
        if Instant::now() >= self.deadline {
            return Err(fail(
                ErrorCode::DeadlineExceeded,
                "the read deadline expired",
            ));
        }
        reader.set_deadline(self.deadline);
        *handle = Some(reader.interrupt_handle());
        Ok(reader)
    }
}

/// The mutation FIFO: tickets `serving..issued` are outstanding. A ticket is issued only once its
/// thread exists, so only a settled head ticket ever advances `serving`.
#[derive(Default)]
pub(crate) struct Queue {
    issued: u64,
    serving: u64,
}

fn is_revocation(c: Command) -> bool {
    matches!(
        c,
        Command::RootsRevoke | Command::GrantsRevoke | Command::AskRevoke | Command::ClientsRevoke
    )
}

/// Revocation must never wait behind other mutations (its state change is serialised by
/// `Inner::mutation`), and the native pickers wait on the owner: none of them takes a ticket.
fn is_ticketed(c: Command) -> bool {
    !is_revocation(c)
        && !matches!(
            c,
            Command::RootsRequestGrant | Command::BackupsImport | Command::DiagnosticsSave
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
/// retryable; `internal_error` at most once, never for `teacher.run`. The hard root cap
/// requires an owner revocation rather than a retry.
pub(crate) fn retry_flag(command: Command, mut body: ErrorBody) -> ErrorBody {
    use ErrorCode::*;
    body.retryable = match body.code {
        ResourceBudget => {
            body.detail
                .as_ref()
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str)
                != Some("root_limit_reached")
        }
        StaleGeneration | DeviceOffline | PartialCoverage | Busy => true,
        DeadlineExceeded => command == Command::PlacementSimulate || !is_mutation(command),
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
        E::GenerationMismatch => with_reason(
            fail(
                ErrorCode::StaleGeneration,
                "the root has a different generation",
            ),
            "generation_changed",
        ),
        E::RepairRequired => fail(ErrorCode::PartialCoverage, "aggregates are being repaired"),
        E::ResourceBudget => fail(ErrorCode::ResourceBudget, "catalogue budget exceeded"),
        E::Cancelled => fail(ErrorCode::Cancelled, "cancelled"),
        E::DatasetMismatch => fail(ErrorCode::PermissionDenied, "dataset_class_mismatch"),
        _ => fail(ErrorCode::InternalError, "catalogue error"),
    }
}

/// #924: the SQLite result codes ride in `detail` (never SQL text, a path or a row), and the full
/// error goes to stderr, so a bare "state store error" is diagnosable after the fact.
pub(crate) fn sql_error(e: rusqlite::Error) -> ErrorBody {
    eprintln!("loomward-service: state store error: {e}");
    let mut body = fail(ErrorCode::InternalError, "state store error");
    if let Some(sqlite) = e.sqlite_error() {
        body.detail = Detail::new(BTreeMap::from([
            (
                "sqlite_code".to_string(),
                json!(format!("{:?}", sqlite.code)),
            ),
            (
                "sqlite_extended_code".to_string(),
                json!(sqlite.extended_code),
            ),
        ]))
        .ok();
    }
    body
}

/// What a handler produced: the result object and the revisions it was read or committed at.
pub(crate) struct Out {
    pub result: Value,
    pub catalog_rev: Option<i64>,
    pub state_rev: Option<i64>,
}

impl ViewService for Service {
    fn response_delivery_policy(
        &self,
        command: Command,
    ) -> Option<loomward_protocol::ResponseDeliveryPolicy> {
        if matches!(command, Command::FeedbackHistory | Command::AlertsHistory) {
            return Some(self.inner.history_delivery.clone());
        }
        matches!(
            command,
            Command::RootsRequestGrant
                | Command::RootsList
                | Command::VolumesDeclareTier
                | Command::VolumesList
                | Command::TiersModel
                | Command::AlertsList
                | Command::AlertsConfigure
        )
        .then(|| self.inner.reply_delivery.clone())
    }

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
        // LW-067: a client principal reaches the grant-scoped reads only, whatever its grant says.
        if matches!(ctx.principal, Principal::Client(_)) && !clients::open_to_clients(command) {
            let e = fail(
                ErrorCode::PermissionDenied,
                "this command is not open to a client",
            );
            return ResponseEnvelope::error(Some(id), e, None);
        }
        // docs/47: copy-and-verify is served for synthetic sessions on registered lab roots (lane C5,
        // copy.rs). A personal session never gets past this point with a copy command until lane C8
        // opens its effect gate (AGENTS.md invariant 1). A personal approval over HTTP is refused by its
        // own rule (G6).
        if command.as_str().starts_with("copy.") && self.inner.dataset == DatasetClass::Personal {
            let personal_http = command == Command::CopyApprove
                && ctx.adapter == loomward_protocol::Adapter::Http
                && self.inner.dataset == DatasetClass::Personal;
            let e = if personal_http {
                with_reason(
                    fail(
                        ErrorCode::CapabilityUnavailable,
                        "a personal copy is approved only in the desktop's native confirmation",
                    ),
                    "copy_personal_approval_http",
                )
            } else {
                with_reason(
                    fail(
                        ErrorCode::CapabilityUnavailable,
                        "copy-and-verify is not open: its effect gate (docs/47 lane C8) is closed",
                    ),
                    "copy_effect_gated",
                )
            };
            return ResponseEnvelope::error(Some(id), e, None);
        }
        // q-11 opens local learning only: no teacher path ever sees personal data (q-5 unmet).
        if self.inner.dataset == DatasetClass::Personal
            && matches!(
                command,
                Command::TeacherPreview
                    | Command::TeacherRun
                    | Command::TeacherResults
                    | Command::GrantsCreateDisclosure
            )
        {
            let e = with_reason(
                fail(
                    ErrorCode::CapabilityUnavailable,
                    "The teacher never receives personal data.",
                ),
                "teacher_personal_gated",
            );
            return ResponseEnvelope::error(Some(id), e, None);
        }
        if command.wave() >= 3
            && !matches!(
                command,
                Command::FeedbackRecord
                    | Command::FeedbackHistory
                    | Command::LearningStatus
                    | Command::LearningQueue
                    | Command::LearningRefit
                    | Command::ThreadsMeaning
                    | Command::CollectionsList
                    | Command::CollectionsCreate
                    | Command::CollectionsUpdateMembers
                    | Command::CollectionsMembers
                    | Command::BackupsList
                    | Command::BackupsImport
                    | Command::BackupsForget
                    | Command::DiagnosticsPreview
                    | Command::DiagnosticsSave
                    | Command::CopyPreview
                    | Command::CopyApprove
                    | Command::CopyStart
                    | Command::CopyCancel
                    | Command::CopyAbandon
                    | Command::CopyStatus
                    | Command::AskStatus
                    | Command::AskConfigure
                    | Command::AskGrant
                    | Command::ClientsGrant
                    | Command::ClientsList
                    | Command::ClientsRevoke
                    | Command::AskRevoke
                    | Command::AskTurn
                    | Command::AskResult
            )
        {
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
        if is_mutation(command)
            && (command != Command::PlacementSimulate
                || request.payload.get("save").and_then(Value::as_bool) == Some(true))
        {
            self.mutate(request, command, ctx.clone(), started, deadline)
        } else {
            #[cfg(test)]
            let deadline = self
                .inner
                .hooks
                .read_clock
                .lock()
                .unwrap()
                .map_or(deadline, |now| now + Duration::from_millis(wait));
            let permit = match ReadPermit::acquire(self.inner.clone()) {
                Some(p) => p,
                None => {
                    return ResponseEnvelope::error(
                        Some(id),
                        retry_flag(
                            command,
                            fail(ErrorCode::Busy, "too many reads are running; retry shortly"),
                        ),
                        None,
                    )
                }
            };
            let inner = self.inner.clone();
            let ctx = ctx.clone();
            let work = Arc::new(ReadWork {
                deadline,
                interrupt: Mutex::new(None),
                repair_on_read: matches!(&ctx.principal, Principal::Owner)
                    && !guarded_observation(&request, command),
            });
            let worker_work = work.clone();
            let metadata_cursor = Arc::new(Mutex::new(None));
            let worker_cursor = metadata_cursor.clone();
            let (tx, rx) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("lw-read".into())
                .spawn(move || {
                    let _permit = permit;
                    let _ = tx.send(inner.respond(
                        &request,
                        command,
                        &ctx,
                        started,
                        Some(&worker_work),
                        Some(&worker_cursor),
                    ));
                    // The reply is out but this worker still holds the service (see `Drop for Service`).
                    #[cfg(test)]
                    inner.hooks.gate(command, "worker_exit");
                });
            if spawned.is_err() {
                return ResponseEnvelope::error(
                    Some(id),
                    fail(ErrorCode::ResourceBudget, "no worker"),
                    None,
                );
            }
            // Tests hold the receiver here and run its clock ahead, so a read whose worker is
            // parked inside a provider expires exactly there instead of racing a short deadline.
            #[cfg(test)]
            let now = {
                self.inner.hooks.gate(command, "read_wait");
                Instant::now() + *self.inner.hooks.read_ahead.lock().unwrap()
            };
            #[cfg(not(test))]
            let now = Instant::now();
            #[cfg(test)]
            let reply = self.inner.hooks.receive_read(&rx, deadline, now);
            #[cfg(not(test))]
            let reply = rx.recv_timeout(deadline.saturating_duration_since(now));
            match reply {
                Ok(mut r) => {
                    if let ResponseEnvelope::Ok(ref mut response) = r {
                        if let Some(position) = metadata_cursor
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .take()
                        {
                            let cursor = self.inner.metadata_cursors.issue(
                                command,
                                String::new(),
                                position,
                                None,
                            );
                            response.result.insert("next_cursor".into(), json!(cursor));
                        }
                    }
                    r
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(handle) = work
                        .interrupt
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_ref()
                    {
                        handle.interrupt();
                    }
                    ResponseEnvelope::error(
                        Some(id),
                        retry_flag(
                            command,
                            fail(
                                ErrorCode::DeadlineExceeded,
                                "the read did not finish within its deadline",
                            ),
                        ),
                        None,
                    )
                }
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
        let busy = |id: RequestId| {
            let e = fail(
                ErrorCode::Busy,
                "too many mutations are pending; retry shortly",
            );
            ResponseEnvelope::error(Some(id), retry_flag(command, e), None)
        };
        // A ticketed mutation claims its key and takes its ticket under the queue lock, so the
        // cap check, the claim and the ticket are one step.
        let mut queue = is_ticketed(command)
            .then(|| self.inner.turn.0.lock().unwrap_or_else(|e| e.into_inner()));
        // Revocation is never refused by the slot cap: other mutations cannot crowd it out.
        let claim = if is_revocation(command) {
            self.inner.idem.claim_uncapped(&id, &fingerprint)
        } else {
            self.inner.idem.claim(&id, &fingerprint)
        };
        match claim {
            idem::Claim::Full => return busy(id),
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
                let ticket = match &queue {
                    Some(q) if q.issued - q.serving >= MAX_PENDING => {
                        self.inner.idem.release(&id);
                        return busy(id);
                    }
                    Some(q) => Some(q.issued),
                    None => None,
                };
                let inner = self.inner.clone();
                let key = id.to_string();
                #[cfg(test)]
                let forced = self
                    .inner
                    .hooks
                    .fail_spawn
                    .swap(false, std::sync::atomic::Ordering::SeqCst);
                #[cfg(not(test))]
                let forced = false;
                let spawned = if forced {
                    Err(std::io::Error::other("forced spawn failure"))
                } else {
                    std::thread::Builder::new()
                        .name("lw-mutation".into())
                        .spawn(move || {
                            let mut settle = Settle {
                                inner: inner.clone(),
                                key,
                                ticket,
                                outcome: None,
                            };
                            if let Some(t) = ticket {
                                let mut queue =
                                    inner.turn.0.lock().unwrap_or_else(|e| e.into_inner());
                                while queue.serving != t {
                                    queue =
                                        inner.turn.1.wait(queue).unwrap_or_else(|e| e.into_inner());
                                }
                            }
                            let work = ReadWork {
                                deadline,
                                interrupt: Mutex::new(None),
                                repair_on_read: true,
                            };
                            // Owner scans and hashing can wait for automatic work to drain.
                            let bounded = matches!(
                                command,
                                Command::PlacementSimulate
                                    | Command::ScanStart
                                    | Command::DuplicatesInspect
                            );
                            settle.outcome = Some(inner.respond(
                                &request,
                                command,
                                &ctx,
                                started,
                                bounded.then_some(&work),
                                None,
                            ));
                            drop(settle);
                            // The outcome is out but this worker still holds the service.
                            #[cfg(test)]
                            inner.hooks.gate(command, "worker_exit");
                        })
                };
                if spawned.is_err() {
                    // No thread: nothing ran. The ticket is never issued (queue order untouched)
                    // and the key is freed, so a retry with the same request_id runs.
                    self.inner.idem.release(&id);
                    let e = retry_flag(command, fail(ErrorCode::ResourceBudget, "no worker"));
                    return ResponseEnvelope::error(Some(id), e, None);
                }
                if let Some(q) = queue.as_mut() {
                    q.issued += 1;
                }
            }
        }
        drop(queue);
        // Tests hold the caller here and run its clock ahead (`Hooks::skew`, the clock
        // `Inner::past` reads), so a mutation parked at a gate expires exactly there (#698).
        #[cfg(test)]
        let deadline = {
            self.inner.hooks.gate(command, "mutate_wait");
            let ahead = *self.inner.hooks.skew.lock().unwrap();
            deadline.checked_sub(ahead).unwrap_or_else(Instant::now)
        };
        self.inner.idem.wait(&id, deadline).unwrap_or_else(|| {
            ResponseEnvelope::error(
                Some(id),
                fail(
                    ErrorCode::DeadlineExceeded,
                    "outcome unknown: retry with the same request_id, or read the current state",
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
    /// Only the head ticket settles (each waits for its turn), so this is `serving += 1`.
    fn advance(inner: &Inner, ticket: u64) {
        let mut queue = inner.turn.0.lock().unwrap_or_else(|e| e.into_inner());
        queue.serving = ticket + 1;
        drop(queue);
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
    #[allow(clippy::too_many_arguments)]
    fn respond(
        &self,
        request: &RequestEnvelope,
        command: Command,
        ctx: &CallContext,
        started: Instant,
        work: Option<&ReadWork>,
        metadata_cursor: Option<&Mutex<Option<cursors::Position>>>,
    ) -> ResponseEnvelope {
        let id = request.request_id.clone();
        // LW-067: the grant is read once per call, so one call never mixes two grants.
        let grant = match &ctx.principal {
            Principal::Owner => None,
            Principal::Client(client) => match self.client_grant(client) {
                Ok(g) => Some(g),
                Err(e) => return ResponseEnvelope::error(Some(id), e, None),
            },
        };
        let out = match &grant {
            None => self.handle(request, command, ctx, work, metadata_cursor),
            Some(g) => self.client_handle(request, command, g, work),
        };
        let out = out.and_then(|out| {
            if grant.is_none() && guarded_observation(request, command) {
                // Composed observations must not carry pre-read revisions across a concurrent commit.
                let (catalog, state) = self.revisions()?;
                if out.catalog_rev != Some(catalog) || out.state_rev != Some(state) {
                    return Err(fail(
                        ErrorCode::StaleGeneration,
                        "Observations or access changed; read again.",
                    ));
                }
            }
            Ok(out)
        });
        let out = out.and_then(|out| match out.result {
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
        // A saved simulation or a started owner job happened: retain its receipt even past the
        // deadline (#565/#568; the handlers bound their own pre-emption waits).
        let out = if work.is_some_and(|work| self.past(work.deadline))
            && !matches!(command, Command::ScanStart | Command::DuplicatesInspect)
            && !(command == Command::PlacementSimulate
                && request.payload.get("save").and_then(Value::as_bool) == Some(true))
        {
            Err(fail(
                ErrorCode::DeadlineExceeded,
                "the read deadline expired",
            ))
        } else {
            out
        };
        match out {
            Ok(Out {
                result: Value::Object(mut map),
                catalog_rev,
                state_rev,
            }) => {
                if let Some(g) = &grant {
                    clients::project(command, g, &mut map);
                    if command.validate_client_result(&map).is_err() {
                        return ResponseEnvelope::error(
                            Some(id),
                            fail(
                                ErrorCode::InternalError,
                                "the service produced an invalid client result",
                            ),
                            None,
                        );
                    }
                }
                let meta = self.meta(started, &map, catalog_rev, state_rev);
                ResponseEnvelope::ok_object(id, map, meta)
            }
            Ok(_) => unreachable!("validated above"),
            Err(e) => {
                let mut e = retry_flag(command, e);
                if command == Command::PlacementSimulate
                    && request.payload.get("save").and_then(Value::as_bool) == Some(true)
                    && e.code == ErrorCode::DeadlineExceeded
                {
                    e.retryable = false;
                }
                ResponseEnvelope::error(Some(id), e, None)
            }
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
            catalog_rev: catalog_rev.and_then(|r| u64::try_from(r).ok().map(Generation)),
            state_rev: state_rev.and_then(|r| u64::try_from(r).ok().map(Generation)),
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
            if event == EventName::VolumesChanged {
                self.invalidate_root_presence();
            }
            let _ = self.engine.publish_event(
                event,
                map,
                catalog_rev.and_then(|r| u64::try_from(r).ok().map(Generation)),
                state_rev.and_then(|r| u64::try_from(r).ok().map(Generation)),
            );
        }
    }

    /// #219: the root volume's filesystem name (NTFS, ReFS, FAT32, exFAT, or whatever the volume
    /// reports; `unknown` when it reports none), observed through the same guarded, non-elevated
    /// open a scan uses. Fails closed.
    pub(crate) fn root_filesystem(&self, root: &Path) -> Result<String, Refusal> {
        self.root_observation(root)
            .map(|(filesystem, _)| filesystem)
    }
    fn root_observation(
        &self,
        root: &Path,
    ) -> Result<(String, loomward_windows::enumerate::OpenedIdentity), Refusal> {
        self.open_observation(root)
            .map_err(|_| Refusal::IdentityUnavailable)
    }
    /// [`Self::root_observation`] keeping the native error, so a caller can tell "not found" from
    /// "could not tell".
    fn open_observation(
        &self,
        root: &Path,
    ) -> Result<
        (String, loomward_windows::enumerate::OpenedIdentity),
        loomward_windows::enumerate::SourceError,
    > {
        #[cfg(windows)]
        {
            use loomward_engine::scan::source::DirSource;
            let source = loomward_windows::enumerate::NativeSource::default();
            #[cfg(test)]
            let source = match *self.hooks.filesystem.lock().unwrap() {
                Some(name) => source.with_filesystem(name),
                None => source,
            };
            let (dir, identity) = source.with_identity_unverified().open_root(root)?;
            Ok((
                match dir.filesystem() {
                    "" => "unknown".into(),
                    name => name.chars().take(32).collect(),
                },
                identity,
            ))
        }
        #[cfg(not(windows))]
        {
            let _ = root;
            Err(loomward_windows::enumerate::SourceError::Unsupported)
        }
    }

    /// Whether a renamed unverified grant's old path is definitely not this folder: a missing
    /// path, a non-directory component (Win32 error or NT status), a plain file (not a link or other
    /// reparse point) where the folder was, or an
    /// opened folder with another identity. Other failures, including attribute or policy
    /// refusals, prove nothing, so the grant stays active and a later re-grant can retire it.
    fn old_path_is_another_folder(&self, old: &Path, identity: &(String, Vec<u8>, String)) -> bool {
        use loomward_windows::enumerate::SourceError;
        #[cfg(test)]
        self.hooks
            .gate(Command::RootsRequestGrant, "old_path_probe");
        #[cfg(test)]
        let injected = *self.hooks.probe_error.lock().unwrap();
        #[cfg(not(test))]
        let injected = None;
        match injected.map_or_else(|| self.open_observation(old), Err) {
            Ok((_, seen)) => Self::identity_triple(&seen).is_ok_and(|seen| seen != *identity),
            Err(SourceError::Io(code)) => {
                matches!(
                    code as u32,
                    2 | 3 | 267 | 0xC000_000F | 0xC000_0034 | 0xC000_003A | 0xC000_0103
                )
            }
            // The native open refuses any non-directory (and reparse points) without saying which;
            // only a plain file, never a link or other reparse point, proves the old path is another
            // object. Everything else stays inconclusive.
            Err(SourceError::Refused) => std::fs::symlink_metadata(old).is_ok_and(|m| {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    m.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = m.file_type().is_symlink();
                m.is_file() && !reparse
            }),
            Err(_) => false,
        }
    }

    fn identity_triple(
        identity: &loomward_windows::enumerate::OpenedIdentity,
    ) -> Result<(String, Vec<u8>, String), Refusal> {
        let file_id = match identity.id {
            Some(loomward_windows::enumerate::FileIdObs::Id128(id)) => id.to_vec(),
            Some(loomward_windows::enumerate::FileIdObs::Id64(id)) => id.to_le_bytes().to_vec(),
            None => return Err(Refusal::IdentityUnavailable),
        };
        Ok((
            format!(
                "vsn_{}",
                identity.volume_serial.ok_or(Refusal::IdentityUnavailable)?
            ),
            file_id,
            format!(
                "{:032x}",
                identity.volume_guid.ok_or(Refusal::IdentityUnavailable)?
            ),
        ))
    }

    /// Grants one already path-checked root under the session's provenance policy. A synthetic
    /// session accepts only a registered lab root whose native identity matches the registry; a
    /// personal session accepts an owner folder. Re-granting an active root returns it.
    pub(crate) fn grant(
        &self,
        root: &Path,
        via: &str,
        confirmed_unverified: bool,
    ) -> Result<(i64, bool), Refused> {
        let (filesystem, identity) = self.root_observation(root)?;
        let (volume_key, file_id, volume_guid) = Self::identity_triple(&identity)?;
        // #219: a personal root on a volume that cannot verify identity needs the owner's
        // explicit confirmation; a synthetic lab root may list unverified freely.
        let unverified = !loomward_windows::enumerate::verifies_identity(&filesystem);
        let text = root.to_string_lossy();
        let display_path = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
        // A grant is consent for one path (docs/41 section 16): a moved root is granted afresh,
        // and the grant still naming its old path ends identity_changed.
        let candidates = {
            let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
            let conn = self.db();
            db::renamed_grants(&conn, &volume_key, &file_id, &display_path, &volume_guid)
                .map_err(|_| Refusal::IdentityUnavailable)?
        };
        // An identity-unverified ID is weak and recyclable, so its grant retires only once the old
        // path is definitely another folder. Slow native observations hold neither service lock.
        let observed: Vec<_> = candidates
            .into_iter()
            .filter(|(_, unverified, old)| {
                !*unverified
                    || self.old_path_is_another_folder(
                        Path::new(old),
                        &(volume_key.clone(), file_id.clone(), volume_guid.clone()),
                    )
            })
            .collect();
        let presence = self.cached_root_presence();
        let mutation = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.db();
        // Recheck consent and candidate identity, state, mode and path after the unlocked probes.
        // ponytail: no refresh stamp; a rename-back/re-grant during the unlocked probe can still
        // retire the refreshed grant. This fails closed; the owner must re-grant.
        let existing = db::active_grant(&conn, &volume_key, &file_id, &display_path, &volume_guid)
            .map_err(|_| Refusal::IdentityUnavailable)?;
        let renamed: Vec<i64> =
            db::renamed_grants(&conn, &volume_key, &file_id, &display_path, &volume_guid)
                .map_err(|_| Refusal::IdentityUnavailable)?
                .into_iter()
                .filter(|row| observed.contains(row))
                .map(|(id, ..)| id)
                .collect();
        if unverified
            && self.dataset == DatasetClass::Personal
            && !confirmed_unverified
            && !existing
                .as_ref()
                .is_some_and(|(_, mode)| mode == "unverified")
        {
            return Err(Refused::IdentityUnverified);
        }
        let origin = match self.dataset {
            DatasetClass::Synthetic => {
                if file_id.len() != 16 {
                    return Err(Refusal::IdentityUnavailable.into());
                }
                let digest = db::lab_root(&conn, &volume_key, &file_id, &volume_guid)
                    .map_err(|_| Refusal::IdentityUnavailable)?
                    .ok_or(Refusal::SyntheticSessionRequiresLabRoot)?;
                lab::log_digest(&digest)?;
                "lab_generated"
            }
            DatasetClass::Personal => "owner_granted",
        };
        if existing.is_none()
            // A moved root's old grant retires below, so it does not count against the limit.
            && db::active_grants(&conn).map_err(|_| Refusal::IdentityUnavailable)?
                - renamed.len() as i64
                >= db::MAX_ACTIVE_ROOTS
        {
            return Err(Refused::RootLimit);
        }
        drop(conn);
        #[cfg(test)]
        if existing.is_some() {
            self.hooks
                .gate(Command::RootsRequestGrant, "grant_observed");
        }
        let display_name = root.file_name().map_or_else(
            || display_path.clone(),
            |n| n.to_string_lossy().into_owned(),
        );
        let fresh = existing.is_none();
        let observation = RootObservation {
            volume_guid: Some(volume_guid),
            volume_key,
            display_name: display_name.chars().take(260).collect(),
            display_path,
            root_file_id: Some(file_id),
            // The volume's real filesystem: NTFS/ReFS keep durable references; the writer marks
            // any other filesystem's grant identity-unverified.
            filesystem: Some(filesystem),
            origin: origin.into(),
            granted_via: via.into(),
            observed_at_ns: now_ns(),
        };
        let command = match existing {
            Some((grant_id, _)) => WriteCommand::RefreshRoot {
                grant_id,
                observation,
            },
            None => WriteCommand::RegisterRoot(observation),
        };
        // #843: retire the grants naming the old path before the new grant exists, under the same
        // mutation guard. A failure answers before anything is registered, so every retry path
        // (the desktop picker included, which refuses an already-granted path) finds the same
        // renamed grants again, and two grants for one folder are never both active.
        for grant in renamed {
            let id = db::root_for_grant(&self.db(), grant)
                .ok()
                .flatten()
                .and_then(|row| loomward_protocol::RootId::new(self.root_id(row)).ok());
            if !id.is_some_and(|id| {
                self.identity_changed_locked(grant, &id, true, &mutation, &presence)
            }) {
                return Err(Refusal::IdentityUnavailable.into());
            }
        }
        let reply = self
            .catalog
            .writer()
            .call(command)
            .map_err(|_| Refusal::IdentityUnavailable)?;
        let WriteReply::Root {
            grant_id, root_id, ..
        } = reply
        else {
            return Err(Refusal::IdentityUnavailable.into());
        };
        self.remember_root_presence(grant_id, handlers::RootPresence::Online);
        Ok((root_id, fresh))
    }

    /// The row a node ID names, after its tag, catalogue instance and incarnation check out.
    /// Unknown, forged and gone IDs are `not_found` (no oracle); an ID from a rebuilt catalogue
    /// carries `catalog_instance_changed`.
    pub(crate) fn node(
        &self,
        id: &str,
        reader: &mut loomward_catalog::Reader,
    ) -> Result<NodeKey, ErrorBody> {
        let (key, born) = self
            .ids
            .open(id, &self.instance)
            .map_err(|r| id_refusal(r, "unknown node"))?;
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

    /// The wire ID of a catalogue root row, bound to this catalogue instance.
    pub(crate) fn root_id(&self, row: i64) -> String {
        self.ids.row_id("rt_", row, &self.instance)
    }

    pub(crate) fn volume_id(&self, row: i64) -> String {
        self.ids.row_id("vo_", row, &self.instance)
    }

    /// The root row a wire ID names; one minted for a rebuilt catalogue never names a row here.
    pub(crate) fn root_row(&self, id: &str) -> Result<i64, ErrorBody> {
        self.ids
            .open_row_id("rt_", id, &self.instance)
            .map_err(|r| id_refusal(r, "unknown root"))
    }

    pub(crate) fn volume_row(&self, id: &str) -> Result<i64, ErrorBody> {
        self.ids
            .open_row_id("vo_", id, &self.instance)
            .map_err(|r| id_refusal(r, "unknown volume"))
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
            } else if let Some(row) = bare_row(s, "rt_").filter(|_| key == "root_id") {
                *s = self.root_id(row);
            } else if let Some(row) = bare_row(s, "vo_").filter(|_| key == "volume_id") {
                *s = self.volume_id(row);
            }
        });
        Ok(())
    }
}

/// `not_found`; an ID minted for an earlier catalogue instance also carries
/// `catalog_instance_changed`. Malformed or foreign spellings get no detail (no oracle).
fn id_refusal(r: ids::Refusal, unknown: &str) -> ErrorBody {
    match r {
        ids::Refusal::Foreign => fail(ErrorCode::NotFound, unknown),
        ids::Refusal::InstanceChanged => with_reason(
            fail(
                ErrorCode::NotFound,
                "the catalogue was rebuilt; refetch from the root",
            ),
            "catalog_instance_changed",
        ),
    }
}

/// The catalogue's internal root or volume spelling (`rt_5`), never a wire ID.
fn bare_row(s: &str, prefix: &str) -> Option<i64> {
    let n = s.strip_prefix(prefix)?;
    n.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| n.parse().ok())?
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

/// Match catalogue cleanup's 0.5s to 30s backoff without blocking telemetry or retaining the service.
fn spawn_backup_scrub_retry(inner: Weak<Inner>) {
    let _ = std::thread::Builder::new()
        .name("lw-backup-scrub".into())
        .spawn(move || {
            let mut delay = Duration::from_millis(500);
            loop {
                std::thread::sleep(delay);
                let Some(inner) = inner.upgrade() else { return };
                if inner.backup_scrub_pending().is_ok_and(|p| p.is_some()) {
                    // A poisoned lock is recovered as revoke and forget do; only real contention waits.
                    let guard = match inner.mutation.try_lock() {
                        Ok(g) => Some(g),
                        Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
                        Err(std::sync::TryLockError::WouldBlock) => None,
                    };
                    if let Some(_mutation) = guard {
                        let _ = inner.scrub_backups(None);
                        delay = (delay * 2).min(Duration::from_secs(30));
                    }
                } else {
                    delay = Duration::from_millis(500);
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

fn stored_grant_catalogue_is_legacy(conn: &rusqlite::Connection) -> Result<bool, String> {
    conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
        .map(|version| version == 1)
        .map_err(|_| "cannot inspect stored grants: catalogue version unavailable".into())
}
