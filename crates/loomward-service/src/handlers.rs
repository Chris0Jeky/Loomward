//! One handler per wave-2 command. Each returns the result object and the revisions it was read
//! or committed at; `Inner::respond` validates the object against the command's result type.

use crate::cursors::{Position, Resume};
use crate::{
    catalog_error, db, fail, now, now_ns, paths, refusal_name, sql_error, ts_ns, with_reason,
    Inner, Out,
};
use loomward_catalog::{
    Basis as CBasis, ChildrenRequest, NodeKey, Reader, SliceRequest, Sort, WriteCommand,
};
use loomward_engine::EngineResult;
use loomward_protocol::*;
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::{json, Value};

mod root_view;

type Handled = Result<Out, ErrorBody>;

fn out(result: Value, revs: Option<(i64, i64)>) -> Out {
    Out {
        result,
        catalog_rev: revs.map(|r| r.0),
        state_rev: revs.map(|r| r.1),
    }
}

/// Engine-only results carry no revision.
fn engine<T: Serialize>(r: EngineResult<T>) -> Handled {
    let value = r.map_err(ErrorBody::from)?;
    Ok(out(
        serde_json::to_value(value).map_err(|_| fail(ErrorCode::InternalError, "encode"))?,
        None,
    ))
}

fn unavailable(what: &str) -> Handled {
    Err(fail(ErrorCode::CapabilityUnavailable, what))
}

fn basis(b: Basis) -> CBasis {
    match b {
        Basis::Logical => CBasis::Logical,
        Basis::Allocated => CBasis::Allocated,
    }
}

pub(crate) fn grant_number(id: &str) -> Result<i64, ErrorBody> {
    id.strip_prefix("gr_")
        .and_then(|n| n.parse().ok())
        .filter(|n: &i64| *n > 0 && format!("gr_{n}") == id)
        .ok_or_else(|| fail(ErrorCode::NotFound, "unknown grant"))
}

/// After a forget (#288) no file of the state directory may still hold the root's name. The rolling
/// `state.start-*.db` copies are dropped (the next open makes a fresh one). The one-off
/// `state.before-v*.db` migration copies are the owner's way back to an older build, so each is
/// scrubbed by exact grant ID instead; unknown copy shapes containing the path are removed.
/// Another grant may name the same folder or a child, so a substring is not a failed known scrub.
/// At restart the forgotten path is deliberately no longer stored: unknown shapes must be removed
/// fail-closed, since their contents cannot be proved unrelated without retaining that private path.
pub(crate) fn forget_backups(dir: &std::path::Path, path: Option<&str>) -> Result<(), ErrorBody> {
    // Every copy is visited even after a failure, so one failed scrub never leaves another copy unswept.
    let mut failed = None;
    let entries = std::fs::read_dir(dir).map_err(|_| backup_scrub_error("directory"))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                failed.get_or_insert_with(|| backup_scrub_error("directory"));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let file = entry.path();
        let copy_name = name
            .strip_suffix("-wal")
            .or_else(|| name.strip_suffix("-shm"))
            .or_else(|| name.strip_suffix("-journal"))
            .unwrap_or(&name);
        let migration = copy_name.starts_with("state.before-v") && copy_name.ends_with(".db");
        let sidecar = copy_name != name;
        // Let SQLite settle migration journals and WALs before removing their sidecars.
        if migration && sidecar && file.with_file_name(copy_name).exists() {
            continue;
        }
        let archive_stem = name
            .strip_suffix(".db")
            .or_else(|| name.strip_suffix(".db-wal"))
            .or_else(|| name.strip_suffix(".db-shm"))
            .or_else(|| name.strip_suffix(".db-journal"));
        let archived = archive_stem.is_some_and(|stem| {
            stem.strip_prefix("catalog.corrupt-")
                .or_else(|| stem.strip_prefix("catalog.before-v1-"))
                .is_some_and(|stamp| !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit()))
        });
        let remove = if migration && !sidecar {
            match scrub_copy(&file, &loomward_catalog::sqlite_dir(dir).join("state.db")) {
                Ok((known, _pin)) => {
                    // SQLite is closed; keep the copy pinned while clearing even stale sidecars.
                    for extension in ["db-wal", "db-shm", "db-journal"] {
                        if let Err(e) = std::fs::remove_file(file.with_extension(extension)) {
                            if e.kind() != std::io::ErrorKind::NotFound {
                                failed.get_or_insert_with(|| backup_scrub_error("migration"));
                            }
                        }
                    }
                    !known && path.is_none_or(|path| holds(&file, path))
                }
                Err(_) => {
                    failed.get_or_insert_with(|| backup_scrub_error("migration"));
                    continue; // the rollback copy is kept
                }
            }
        } else {
            (copy_name.starts_with("state.start-") && copy_name.ends_with(".db"))
                || (migration && sidecar)
                || archived
        };
        if remove {
            if let Err(e) = std::fs::remove_file(&file) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    failed.get_or_insert_with(|| {
                        backup_scrub_error(if archived {
                            "catalog_archive"
                        } else if migration {
                            "migration"
                        } else {
                            "startup"
                        })
                    });
                }
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

fn backup_scrub_error(kind: &str) -> ErrorBody {
    let mut error = fail(ErrorCode::InternalError, &format!(
            "the root was forgotten, but {kind} backup cleanup is pending; Loomward will retry automatically and at restart"
        ));
    error.detail = Detail::new(std::collections::BTreeMap::from([
        ("reason".into(), json!("forget_backup_scrub_failed")),
        ("copy_kind".into(), json!(kind)),
    ]))
    .ok();
    error
}

fn live_scrub_error() -> ErrorBody {
    with_reason(fail(ErrorCode::InternalError,
        "the root was forgotten, but database cleanup is still pending; Loomward will retry automatically and at restart"),
        "forget_scrub_pending")
}

/// Refuse aliases, and on Windows pin the checked path against replacement until SQLite closes.
fn pin_backup(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            // READ_DATA makes Windows enforce the share restriction; no content is read here.
            .access_mode(0x81)
            .share_mode(3)
            .custom_flags(0x00300000); // no DELETE sharing; OPEN_REPARSE_POINT|OPEN_NO_RECALL
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.file_type().is_symlink() || paths::unsafe_attributes(&meta) {
        return Err(std::io::Error::other(
            "backup is not an ordinary local file",
        ));
    }
    #[cfg(windows)]
    if loomward_windows::linkcount::from_file(&file)? != 1 {
        return Err(std::io::Error::other("backup has aliases"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let named = std::fs::symlink_metadata(path)?;
        if named.file_type().is_symlink()
            || named.ino() != meta.ino()
            || named.dev() != meta.dev()
            || meta.nlink() != 1
        {
            return Err(std::io::Error::other("backup has aliases or changed"));
        }
    }
    Ok(file)
}

/// Removes from an older-schema copy what the forget removed from `state.db`: every reference
/// `state.db` no longer has (only a forget deletes references) with the rows keyed to it, and the
/// path and identity of every forgotten grant. Freed pages are zeroed and VACUUM rewrites the file.
fn scrub_copy(
    copy: &std::path::Path,
    state: &std::path::Path,
) -> rusqlite::Result<(bool, std::fs::File)> {
    let pin = pin_backup(copy).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let conn = rusqlite::Connection::open(loomward_catalog::sqlite_dir(copy))?;
    // Allow transient store contention to settle before reporting a failed scrub.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // Checkpoint old WAL frames before scrubbing; a held reader keeps cleanup pending.
    conn.execute_batch("PRAGMA main.journal_mode=DELETE")?;
    conn.execute(
        "ATTACH DATABASE ?1 AS live",
        [state.to_string_lossy().as_ref()],
    )?;
    conn.execute_batch("PRAGMA main.secure_delete=ON")?;
    let has = |table: &str, column: &str| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name=?2)",
            [table, column],
            |r| r.get::<_, bool>(0),
        )
    };
    let known = has("root_grant", "display_path")? && has("object_ref", "id")?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if has("root_grant", "display_path")? && version < 2 {
        // v1 state had no grants; an extended/unknown table cannot promise the live ID namespace.
        return Ok((false, pin));
    }
    let tx = conn.unchecked_transaction()?;
    if has("object_ref", "id")? {
        tx.execute_batch("CREATE TEMP TABLE gone AS SELECT id FROM main.object_ref WHERE id NOT IN (SELECT id FROM live.object_ref)")?;
        if has("learning_receipt", "client_event_id")? && has("human_feedback", "client_event_id")?
        {
            tx.execute_batch("DELETE FROM main.learning_receipt WHERE client_event_id IN (SELECT client_event_id FROM main.human_feedback WHERE object_ref_id IN (SELECT id FROM gone))")?;
        }
        let keyed: Vec<String> = tx
            .prepare("SELECT m.name FROM main.sqlite_schema m WHERE m.type='table' AND EXISTS(SELECT 1 FROM pragma_table_info(m.name,'main') WHERE name='object_ref_id')")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for table in keyed {
            // A name read from the copy is an identifier: its own quotes are doubled, so it can
            // never end the quoted name and run as SQL.
            let table = table.replace('"', "\"\"");
            tx.execute_batch(&format!(
                "DELETE FROM main.\"{table}\" WHERE object_ref_id IN (SELECT id FROM gone)"
            ))?;
        }
        tx.execute_batch(
            "DELETE FROM main.object_ref WHERE id IN (SELECT id FROM gone); DROP TABLE gone",
        )?;
    }
    if known {
        tx.execute_batch("UPDATE main.root_grant SET display_path='forgotten',volume_key='forgotten',root_file_id=zeroblob(8) WHERE id IN (SELECT grant_id FROM live.root_forgotten)")?;
        let uncleared: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.root_grant WHERE id IN (SELECT grant_id FROM live.root_forgotten)
             AND (display_path!='forgotten' OR volume_key!='forgotten' OR root_file_id!=zeroblob(8)))", [], |r| r.get(0)
        )?;
        if uncleared {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    tx.commit()?;
    conn.execute_batch("DETACH DATABASE live; VACUUM")?;
    Ok((known, pin))
}

/// Unknown legacy copy shapes need a byte check; buffer only a chunk and the longest overlap.
pub(crate) fn holds(file: &std::path::Path, needle: &str) -> bool {
    std::fs::File::open(file).map_or(true, |file| holds_reader(file, needle))
}

fn holds_reader(mut reader: impl std::io::Read, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let utf16: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let patterns = [needle.as_bytes(), utf16.as_slice()];
    let overlap = patterns.iter().map(|p| p.len()).max().unwrap() - 1;
    let mut bytes = vec![0; 65536 + overlap];
    let mut kept = 0;
    loop {
        let n = match reader.read(&mut bytes[kept..kept + 65536]) {
            Ok(0) => return false,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        };
        let end = kept + n;
        if patterns
            .iter()
            .any(|p| bytes[..end].windows(p.len()).any(|w| w == *p))
        {
            return true;
        }
        kept = overlap.min(end);
        bytes.copy_within(end - kept..end, 0);
    }
}

pub(crate) fn encode<T: Serialize>(v: &T) -> Result<Value, ErrorBody> {
    serde_json::to_value(v).map_err(|_| fail(ErrorCode::InternalError, "encode"))
}

/// The request with its paging fields removed: the binding a cursor must match (section 5).
fn binding(request: &RequestEnvelope) -> String {
    let mut p = request.payload.clone();
    p.remove("cursor");
    p.remove("limit");
    Value::Object(p).to_string()
}

/// A request timestamp as FILETIME; one before 1601 is out of range.
fn ft(t: &Timestamp) -> Result<i64, ErrorBody> {
    loomward_catalog::filetime(t.as_ref())
        .ok_or_else(|| fail(ErrorCode::InvalidRequest, "timestamp out of range"))
}

/// A root's scan state and coverage as `roots.list` shows them; a root that is not active is stale.
pub(crate) fn scan_view(r: &db::RootRow) -> (&'static str, &'static str) {
    if r.grant_state != "active" {
        return ("stale", "stale");
    }
    match r.state.as_str() {
        "never_scanned" => ("never_scanned", "unscanned"),
        "scanning" => ("scanning", "partial"),
        // A finished traversal with excluded, denied or skipped entries is not complete
        // coverage: the root reads partial, as its tree does.
        "complete" if r.subtree_complete => ("complete", "complete"),
        "complete" => ("complete", "partial"),
        "partial" | "repairing" => ("partial", "partial"),
        "stale" => ("stale", "stale"),
        _ => ("failed", "unknown"),
    }
}

fn stale(message: &str, reason: &str) -> ErrorBody {
    with_reason(fail(ErrorCode::StaleGeneration, message), reason)
}

impl Inner {
    pub(crate) fn backup_scrub_pending(&self) -> Result<Option<String>, ErrorBody> {
        self.db()
            .query_row(
                "SELECT value FROM st.meta WHERE key='forget_backup_scrub_pending'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_error)
    }

    /// Called under the mutation lock (or before the service is exposed at startup).
    pub(crate) fn scrub_backups(&self, path: Option<&str>) -> Result<(), ErrorBody> {
        // A later forget must not clear an earlier path's pending cleanup with its own byte check.
        let path = if self.backup_scrub_pending()?.is_some() {
            None
        } else {
            path
        };
        let result = forget_backups(&self.state_dir, path);
        match &result {
            Ok(()) => {
                self.db()
                    .execute(
                        "DELETE FROM st.meta WHERE key='forget_backup_scrub_pending'",
                        [],
                    )
                    .map_err(sql_error)?;
            }
            Err(error) => {
                let kind = error
                    .detail
                    .as_ref()
                    .and_then(|d| d.get("copy_kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("directory");
                self.db().execute(
                    "INSERT INTO st.meta(key,value) VALUES('forget_backup_scrub_pending',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    [kind],
                ).map_err(sql_error)?;
            }
        }
        result
    }

    pub(crate) fn handle(
        &self,
        req: &RequestEnvelope,
        command: Command,
        ctx: &CallContext,
        work: Option<&crate::ReadWork>,
        metadata_cursor: Option<&std::sync::Mutex<Option<Position>>>,
    ) -> Handled {
        #[cfg(test)]
        self.hooks.before(command);
        use Command::*;
        if self.dataset != DatasetClass::Personal
            && matches!(
                command,
                TelemetrySubscribe
                    | TelemetryUnsubscribe
                    | TelemetrySnapshot
                    | ProcessesList
                    | ProcessesExplain
            )
        {
            return unavailable("process observation requires a personal session");
        }
        let handled = match command {
            SessionHello => self.hello(ctx),
            HealthGet => self.health(work),
            AppsList | DesktopContexts => self.metadata_page(req, command, work, metadata_cursor),
            RootsList => self.roots_list(work),
            RootsRequestGrant => self.request_grant(ctx),
            BackupsList => self.backups_list(),
            BackupsImport => self.backups_import(req, ctx),
            BackupsForget => self.backups_forget(req),
            DiagnosticsPreview => self.diagnostics_preview(ctx),
            DiagnosticsSave => self.diagnostics_save(req, ctx),
            // docs/47 C5: only synthetic sessions reach here (personal ones are refused in `call`).
            CopyPreview => self.copy_preview(req),
            CopyApprove => self.copy_approve(req),
            CopyStart => self.copy_start(req),
            CopyCancel | CopyAbandon => self.copy_cancel(req),
            CopyStatus => self.copy_status(req),
            AlertsList => self.alerts_list(work),
            AlertsConfigure => self.alerts_configure(req.decode_payload()?),
            AskStatus => self.ask_status(),
            AskConfigure => self.ask_configure(req.decode_payload()?),
            AskGrant => self.ask_grant(req.decode_payload()?),
            AskRevoke => self.ask_revoke(),
            AskTurn => self.ask_turn(req.decode_payload()?),
            AskResult => self.ask_result(req.decode_payload()?),
            RootsRevoke => {
                let r: RootRevokeRequest = req.decode_payload()?;
                let grant = self.grant_of_root(self.root_row(&r.root_id)?)?;
                let (at, revs) = self.revoke(grant)?;
                Ok(out(
                    json!({"root_id": r.root_id, "revoked_at": at, "purged": false}),
                    Some(revs),
                ))
            }
            RootsForget => self.roots_forget(req),
            GrantsRevoke => {
                let r: GrantRevokeRequest = req.decode_payload()?;
                let (at, revs) = self.revoke(grant_number(&r.grant_id)?)?;
                Ok(out(
                    json!({"grant_id": r.grant_id, "revoked_at": at}),
                    Some(revs),
                ))
            }
            GrantsList => self.grants_list(),
            VolumesList => self.placement_command(req, command, work),
            VolumesDeclareTier => self.declare_tier(req.decode_payload()?, req.expected_state_rev),
            CollectionsList => self.collections_list(work),
            CollectionsCreate => self.collections_create(req),
            CollectionsUpdateMembers => self.collections_update(req),
            CollectionsMembers => self.collections_members(req, work),
            DuplicatesInspect => self.duplicates_inspect(req, work),
            DuplicatesCancel => self.duplicates_cancel(req.decode_payload()?),
            ScanStart => self.scan_start(req.decode_payload()?, work),
            ScanCancel => engine(
                self.engine
                    .scan_cancel(&req.decode_payload::<JobRefRequest>()?.job_id)
                    .map(|job| JobResult { job }),
            ),
            JobsGet => engine(
                self.engine
                    .job_status(&req.decode_payload::<JobRefRequest>()?.job_id)
                    .map(|job| JobResult { job }),
            ),
            JobsList => engine(self.engine.job_list(&req.decode_payload()?)),
            JobsCancel => engine(
                self.engine
                    .job_cancel(&req.decode_payload::<JobRefRequest>()?.job_id)
                    .map(|mut job| {
                        job.duplicates = None;
                        JobResult { job }
                    }),
            ),
            FeedbackRecord => self.feedback_record(req.decode_payload()?),
            LearningStatus => self.learning_status(),
            LearningQueue => self.learning_queue(req.decode_payload()?, work),
            LearningRefit => self.learning_refit(),
            ThreadsMeaning => self.meaning(req.decode_payload()?, work),
            TreeSlice => self.tree_slice(req, work),
            TreeChildren => self.tree_children(req, work),
            TreePath => self.node_read(req, work, |reader, key| {
                reader.path(key).map(|p| encode(&p))
            }),
            NodeInspect => self.node_read(req, work, |reader, key| {
                reader.inspect(key).map(|d| encode(&d))
            }),
            SearchQuery => self.search(req, work, None),
            StatsBreakdown => self.breakdown(req, work),
            ClutterDeveloper => self.clutter_developer(req, work),
            TiersModel | PlacementCandidates | PlacementSimulate | ProposalsList | ProposalsGet => {
                self.placement_command(req, command, work)
            }
            TelemetrySubscribe => engine(self.engine.telemetry_lease(&req.decode_payload()?)),
            TelemetryUnsubscribe => engine(self.engine.telemetry_release(&req.decode_payload()?)),
            TelemetrySnapshot => engine(self.engine.telemetry_snapshot(&req.decode_payload()?)),
            ProcessesList => engine(self.engine.processes_list(&req.decode_payload()?)),
            ProcessesExplain => engine(self.engine.processes_explain(&req.decode_payload()?)),
            BudgetsGet => engine(self.engine.budgets_get()),
            BudgetsSet => engine(self.engine.budgets_set(&req.decode_payload()?)),
            _ => unavailable("this command arrives with wave 3 (lane L20)"),
        };
        #[cfg(test)]
        self.hooks.gate(command, "after");
        handled
    }

    /// `scan.start` (docs/41 section 7): a synthetic session scans its registered lab roots
    /// only; a personal session scans only the folders its owner granted (picker or CLI), never
    /// a lab root. Metadata only: personal scans opened once gates #184, #206, #240 and #269
    /// closed. The engine mints the root from the active grant row and checks its identity; the
    /// job's events (`job.state`, final `scan.progress`, `tree.invalidated`) follow the commits.
    fn scan_start(&self, r: ScanStartRequest, work: Option<&crate::ReadWork>) -> Handled {
        #[cfg(windows)]
        if let Some(inner) = self.weak.upgrade() {
            crate::freshness::ensure_roots(&inner);
        }
        let row = self.root_row(&r.root_id)?;
        let root = db::roots(&self.db(), Some(row))
            .map_err(sql_error)?
            .into_iter()
            .next()
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?;
        if root.grant_state != "active" {
            return Err(fail(
                ErrorCode::PermissionDenied,
                "the root's grant is not active",
            ));
        }
        self.session_root_gate(&root)?;
        let dir = root
            .dir
            .ok_or_else(|| fail(ErrorCode::InternalError, "root has no directory row"))?;
        let id = RootId::new(self.root_id(row)).expect("rt_ id");
        self.scan.bind(&id, root.grant, dir);
        let granted = self.engine.granted_root(
            id.clone(),
            root.grant,
            &loomward_engine::scan::CatalogRootIdentity {
                row,
                instance: self.instance.clone(),
            },
        );
        if matches!(&granted, Err(loomward_engine::EngineError::PermissionDenied { message })
            if message == "root_identity_changed")
        {
            self.identity_changed(root.grant, &id, true);
        }
        let granted = granted?;
        let start = || match r.mode {
            ScanStartRequestMode::Full => self.engine.scan_start(&granted, r.budget.clone()),
            ScanStartRequestMode::Refresh => self.engine.scan_refresh(&granted, r.budget.clone()),
        };
        // Automatic work never refuses the owner's scan: fence all automatic starts, pre-empt
        // this root's automatic job or automatic jobs filling the shared pool. Other roots stay
        // stale until their own full pass completes. The brief start/revoke fence (busy without a job)
        // is waited out; a revoke then answers permission_denied. An explicit scan stays busy.
        let claim = self.freshness.explicit(row);
        let automatic = self.freshness.automatic_jobs();
        let active = |id| {
            self.engine.job_status(id).is_ok_and(|j| {
                use loomward_protocol::JobState::*;
                matches!(j.state, Queued | Running | CancelRequested)
            })
        };
        let deadline = work.map_or_else(
            || std::time::Instant::now() + std::time::Duration::from_secs(5),
            |w| w.deadline,
        );
        let mut job = start();
        loop {
            match &job {
                Err(loomward_engine::EngineError::ResourceBudget { .. }) => {
                    let mut stopping = false;
                    for auto in &automatic {
                        if active(auto) {
                            let _ = self.engine.scan_cancel(auto);
                            stopping = true;
                        }
                    }
                    if !stopping {
                        break;
                    }
                }
                Err(loomward_engine::EngineError::Busy { detail, .. }) => {
                    let holder = detail.as_ref().and_then(|d| d.get("job_id")?.as_str());
                    match &claim.auto {
                        _ if detail.is_none() => {}
                        // Once the automatic job is terminal, the retry below starts the scan.
                        Some(auto) if holder == Some(auto.as_str()) => {
                            if active(auto) {
                                let _ = self.engine.scan_cancel(auto);
                            }
                        }
                        _ => break,
                    }
                }
                _ => break,
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            job = start();
            #[cfg(test)]
            if self
                .hooks
                .fail_explicit_retry
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                job = Err(loomward_engine::EngineError::Internal {
                    message: "synthetic retry failure".into(),
                    detail: None,
                });
            }
        }
        drop(claim);
        // Keep pool exhaustion distinct inside the pre-emption loop; expose contention as Busy.
        let job = job.map_err(|error| match error {
            loomward_engine::EngineError::ResourceBudget { message } => {
                loomward_engine::EngineError::Busy {
                    message,
                    detail: None,
                }
            }
            other => other,
        })?;
        Ok(out(encode(&JobResult { job })?, None))
    }

    /// The root a session may scan or read (scan.start, duplicates.inspect): a synthetic session
    /// its registered lab roots, a personal session its own owner grants that still pass today's
    /// fences. Runs before any engine call or filesystem access.
    pub(crate) fn session_root_gate(&self, root: &db::RootRow) -> Result<(), ErrorBody> {
        let (origin, message, reason) = match self.dataset {
            DatasetClass::Synthetic => (
                "lab_generated",
                "a synthetic session scans registered lab roots only",
                "synthetic_session_requires_lab_root",
            ),
            DatasetClass::Personal => (
                "owner_granted",
                "a personal session scans the folders you granted only",
                "personal_session_requires_owner_grant",
            ),
        };
        if root.origin != origin
            || (self.dataset == DatasetClass::Synthetic
                && !self
                    .db()
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM st.lab_root l JOIN st.root_grant g
                         ON l.volume_key=g.volume_key AND l.root_file_id=g.root_file_id
                         AND l.volume_guid=g.volume_guid WHERE g.id=?1)",
                        [root.grant],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(sql_error)?)
        {
            return Err(with_reason(
                fail(ErrorCode::PermissionDenied, message),
                reason,
            ));
        }
        if self.dataset == DatasetClass::Personal {
            self.stored_grant_fence(root).map_err(|r| {
                with_reason(
                    fail(
                        ErrorCode::PermissionDenied,
                        "the granted folder is not eligible under the current rules",
                    ),
                    &refusal_name(r),
                )
            })?;
        }
        Ok(())
    }

    /// #312: a stored grant may predate today's fences (#253, #284, #286), so the path rules run
    /// again before any engine call: the profile, profiles-directory and store fence, the
    /// state-directory overlap, and a volume root only from the native picker. Fails closed.
    /// The engine's identity check still binds the scan to the grant's folder after this.
    fn stored_grant_fence(&self, root: &db::RootRow) -> Result<(), RootGrantResultRefusal> {
        let profile = |p: &std::path::Path| {
            #[cfg(test)]
            if let Some((profile, profiles)) = self.hooks.profile.lock().unwrap().clone() {
                return paths::check_profile_with(p, Some(profile), Some(profiles));
            }
            paths::check_profile(p)
        };
        paths::check_root_with(
            std::path::Path::new(&root.display_path),
            Some(&self.state_dir),
            &[],
            root.granted_via == "desktop_picker",
            profile,
        )
        .map(drop)
    }

    pub(crate) fn hello(&self, ctx: &CallContext) -> Handled {
        let personal = self.dataset == DatasetClass::Personal;
        let f = false;
        Ok(out(
            json!({
                "protocol": "loomward/3",
                "engine_version": concat!("loomward-service/", env!("CARGO_PKG_VERSION")),
                "adapter": ctx.adapter,
                "dataset_class": self.dataset,
                "session_started_at": self.started_at,
                "enumeration_strategy": "file_id_extd_directory_info",
                "capabilities": {
                    "observation": {
                        // Honest: metadata scans run on Windows only (lab roots in a synthetic
                        // session, granted folders in a personal one); telemetry observes
                        // personal hosts only; the teacher stays disabled (confinement_not_enforced).
                        "metadata_scan": cfg!(windows),
                        "duplicate_content_read": cfg!(windows),
                        "process_observation": personal,
                        "gpu_observation": f,
                        "disk_io_observation": personal,
                        "teacher_disclosure": f,
                    },
                    "effects": {
                        "file_move": f, "file_delete": f, "file_rename": f, "file_write": f,
                        "content_read": f, "process_kill": f, "process_suspend": f,
                        "process_priority": f, "memory_trim": f, "uninstall": f, "elevation": f,
                    },
                },
                "features": {
                    "grant_picker": personal && ctx.dialogs.is_some(),
                    "disclosure_dialog": f,
                    "teacher_available": f,
                    "telemetry_available": personal,
                    "gpu_available": f,
                    // Every session may save its redacted bundle where a native dialog exists.
                    "diagnostics_save": ctx.dialogs.is_some(),
                    // The lane that serves the federated fields (F2 to F4) flips this constant,
                    // and the guard in `search` goes with it.
                    "federated_search": crate::FEDERATED_SEARCH,
                },
                "limits": {
                    "max_request_bytes": limits::MAX_REQUEST_BYTES,
                    "max_slice_nodes": limits::MAX_SLICE_NODES,
                    "max_page_items": limits::MAX_PAGE_ITEMS,
                },
            }),
            None,
        ))
    }

    /// `Health.engine` is read now from the process itself (#163), never a retained telemetry
    /// sample under a fresh `observed_at`. Unknown stays `null`.
    pub(crate) fn health(&self, work: Option<&crate::ReadWork>) -> Handled {
        let own = match loomward_telemetry::own_usage() {
            loomward_telemetry::Observation::Observed { value } => Some(value),
            _ => None,
        };
        let size = |name: &str| {
            std::fs::metadata(self.state_dir.join(name))
                .ok()
                .map(|m| m.len().to_string())
        };
        let (files, dirs) = self
            .read_reader(work)?
            .entry_counts()
            .map_err(catalog_error)?;
        let jobs = self.engine.active_job_count();
        #[cfg(test)]
        let jobs = if self
            .hooks
            .fail_job_list
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            Err(loomward_engine::EngineError::Internal {
                message: "synthetic job list failure".into(),
                detail: None,
            })
        } else {
            jobs
        };
        let running = jobs.map_err(ErrorBody::from)?;
        let revs = self.revisions()?;
        let scrub_pending: bool = self
            .db()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM st.meta WHERE key='scrub_pending')",
                [],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        let last_error = self
            .backup_scrub_pending()?
            .map(|kind| backup_scrub_error(&kind))
            .or_else(|| scrub_pending.then(live_scrub_error));
        Ok(out(
            json!({
                "observed_at": now(),
                "engine": {
                    "private_commit_bytes": own.as_ref().map(|o| o.private_commit_bytes.to_string()),
                    "working_set_bytes": null,
                    "cpu_seconds": own.as_ref().map(|o| o.cpu_seconds),
                    "threads": null,
                },
                "catalog": {
                    "schema_version": loomward_catalog::SCHEMA_VERSION,
                    "db_bytes": size("catalog.db"),
                    "wal_bytes": size("catalog.db-wal"),
                    "files": files,
                    "dirs": dirs,
                    // The writer queue is bounded in bytes; depth and capacity are both bytes.
                    "writer_queue_depth": self.catalog.writer().queued_bytes(),
                    "writer_queue_capacity": loomward_catalog::QUEUE_BYTES,
                },
                "jobs_running": running,
                "last_error": last_error,
                "warnings": [],
            }),
            Some(revs),
        ))
    }

    fn root_list(&self, work: Option<&crate::ReadWork>) -> Result<Value, ErrorBody> {
        self.root_list_sharing(work, 75)
    }

    /// `share_percent` of the time left to the deadline goes to the roots' unique totals.
    pub(crate) fn root_list_sharing(
        &self,
        work: Option<&crate::ReadWork>,
        share_percent: u32,
    ) -> Result<Value, ErrorBody> {
        let rows = db::roots(&self.db(), None).map_err(sql_error)?;
        let mut reader = self.read_reader(work)?;
        // Unique totals share the request's budget (#722): keep the rest of it (a quarter) for the
        // response, split the remainder over the roots still to measure, and let time a small
        // root leaves flow to the next. A root past its share reports unknown totals, not a
        // failed list.
        let budget_end = work.map(|w| {
            let now = std::time::Instant::now();
            now + w.deadline.saturating_duration_since(now) * share_percent / 100
        });
        let measured = |r: &db::RootRow| {
            r.dir.is_some() && r.grant_state == "active" && r.state != "never_scanned"
        };
        let mut left = rows.iter().filter(|r| measured(r)).count();
        let roots = rows
            .iter()
            .map(|r| {
                if let Some(end) = budget_end {
                    let now = std::time::Instant::now();
                    reader.unique_totals_cut =
                        Some(now + end.saturating_duration_since(now) / left.max(1) as u32);
                    if measured(r) {
                        left -= 1;
                    }
                }
                self.root_json(r, Some(&mut reader))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({"roots": roots}))
    }

    fn roots_list(&self, work: Option<&crate::ReadWork>) -> Handled {
        let revs = self.revisions()?;
        Ok(out(self.root_list(work)?, Some(revs)))
    }

    fn grant_of_root(&self, root: i64) -> Result<i64, ErrorBody> {
        db::roots(&self.db(), Some(root))
            .map_err(sql_error)?
            .first()
            .map(|r| r.grant)
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))
    }

    /// Revocation (docs/41 section 12): commit in `state.db` at FULL first (the catalogue writer
    /// commits the grant row before it cancels the root's run and refuses any later message for
    /// the grant), then cancel the engine's jobs for the root, then fence the engine's writer, and
    /// only then publish `roots.changed`. Monotone: a revoked grant returns its original time.
    pub(crate) fn revoke(&self, grant: i64) -> Result<(Timestamp, (i64, i64)), ErrorBody> {
        let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let (state, revoked_at): (String, Option<i64>) = self
            .db()
            .query_row(
                "SELECT state,revoked_at_ns FROM st.root_grant WHERE id=?1",
                [grant],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown grant"))?;
        if state != "active" && state != "identity_changed" {
            // A prior revoke may have committed its grant but failed feature cleanup.
            self.engine
                .learning_redact_revoked()
                .map_err(ErrorBody::from)?;
            let at = revoked_at.ok_or_else(|| {
                fail(
                    ErrorCode::PermissionDenied,
                    "the grant's root identity changed",
                )
            })?;
            self.copies_fence(grant)?;
            return Ok((ts_ns(at), self.revisions()?));
        }
        let at = now_ns();
        self.catalog
            .writer()
            .call(WriteCommand::RevokeGrant {
                grant_id: grant,
                revoked_at_ns: at,
            })
            .map_err(catalog_error)?;
        // Copies using the grant are fenced next (docs/47 section 5): no effect step starts after.
        let copy_fence = self.copies_fence(grant);
        let root = db::root_for_grant(&self.db(), grant).map_err(sql_error)?;
        if let Some(root) = root {
            self.freshness.remove(root);
            let id = RootId::new(self.root_id(root)).expect("rt_ id");
            // The revocation is durable whatever happens next; a missing scan writer or job only
            // means there is nothing to cancel or fence.
            let _ = self
                .engine
                .scan_cancel_root(&loomward_engine::scan::RootKey::Grant(grant), &id);
            let _ = self
                .engine
                .scan_writer_fence(&loomward_engine::scan::RootKey::Grant(grant), &id);
        }
        let redaction = self
            .engine
            .learning_redact_revoked()
            .map_err(ErrorBody::from);
        let revs = self.revisions()?;
        if let Ok(list) = self.root_list(None) {
            self.publish(EventName::RootsChanged, list, Some(revs.0), Some(revs.1));
        }
        copy_fence?;
        redaction?;
        Ok((ts_ns(at), revs))
    }

    /// Owner decision q-9: a deliberate, user-initiated purge of one revoked root from Loomward's
    /// own state, never from disk. Active roots are refused; the writer does the work (state half
    /// first, then the catalogue half) and keeps an anonymised grant record. A mismatch of the
    /// optional `expected_state_rev` writes nothing.
    fn roots_forget(&self, req: &RequestEnvelope) -> Handled {
        let r: RootForgetRequest = req.decode_payload()?;
        let root = self.root_row(&r.root_id)?;
        // Declare before the mutation guard: on every return or unwind it wakes adapters only
        // after the service lock releases. It is initialized only once forget can reach a write.
        let reply_delivery;
        let mutation = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let (grant, grant_state, path, volume) = db::roots(&self.db(), Some(root))
            .map_err(sql_error)?
            .first()
            .map(|r| {
                (
                    r.grant,
                    r.grant_state.clone(),
                    r.display_path.clone(),
                    r.volume,
                )
            })
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?;
        if grant_state != "revoked" {
            return Err(with_reason(
                fail(
                    ErrorCode::InvalidRequest,
                    "only a revoked root can be forgotten; revoke it first",
                ),
                if grant_state == "active" {
                    "root_active"
                } else {
                    "root_not_revoked"
                },
            ));
        }
        let at = now_ns();
        // The state half can commit before the catalogue purge returns. Fence adapter admission
        // for that whole interval without holding a Bus lock over the blocking writer call.
        let _delivery = self.engine.suspend_event_delivery();
        reply_delivery = self.reply_delivery.suspend();
        let written = self.catalog.writer().call(WriteCommand::ForgetGrant {
            grant_id: grant,
            forgotten_at_ns: at,
            expected_state_rev: req.expected_state_rev.map(|g| g.get() as i64),
        });
        #[cfg(test)]
        self.hooks.gate(Command::RootsForget, "forget_written");
        // A derived purge error does not undo state.db's FULL forget transaction. Read its marker
        // before propagating the original error. If that read also fails, suppress the old name
        // conservatively; an unreadable marker cannot establish permission to deliver it.
        let forgotten = written.is_ok()
            || self
                .db()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM st.root_forgotten WHERE grant_id=?1)",
                    [grant],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(true);
        if forgotten {
            self.engine.redact_root_events(r.root_id.as_str());
            let volume_id = volume.map(|id| self.volume_id(id));
            self.idem
                .forget_root(r.root_id.as_str(), volume_id.as_deref());
            if let Some(volume_id) = volume_id {
                self.engine.redact_volume_events(&volume_id);
            }
        } else {
            // The writer rejected before its durable marker committed. Resume this response
            // generation without withdrawing successful receipts. Durable/unknown outcomes and
            // every unclassified unwind keep the guard's fail-closed retirement on Drop.
            // A false marker implies a writer error; release before waking, then return that
            // original error below without any further owner-state work.
            drop(mutation);
            reply_delivery.resume();
        }
        written.map_err(|e| match e {
            loomward_catalog::Error::StaleGeneration => stale(
                "state.db changed since your read; re-read and ask the owner again",
                "state_rev_changed",
            ),
            e => catalog_error(e),
        })?;
        self.db()
            .execute("DELETE FROM st.root_freshness WHERE grant_id=?1", [grant])
            .map_err(sql_error)?;
        let backups = self.scrub_backups(Some(&path));
        let revs = self.revisions()?;
        if let Ok(list) = self.root_list(None) {
            self.publish(EventName::RootsChanged, list, Some(revs.0), Some(revs.1));
        }
        // The live forget is committed and announced; only a kept migration copy may still hold the name.
        backups?;
        let scrub_pending: bool = self
            .db()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM st.meta WHERE key='scrub_pending')",
                [],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        if scrub_pending {
            return Err(live_scrub_error());
        }
        Ok(out(
            json!({"root_id": r.root_id, "forgotten_at": ts_ns(at)}),
            Some(revs),
        ))
    }
    /// The granted folder no longer has the grant's identity (docs/41 section 7 step 1): the
    /// grant ends `identity_changed`, committed in `state.db` first as a revocation is; then the
    /// root's jobs are cancelled and its writer fenced, and `roots.changed` stops the UI from
    /// advertising the dead grant.
    /// `cancel` is false when the failing scan job itself reports it (nothing else runs).
    pub(crate) fn identity_changed(&self, grant: i64, id: &RootId, cancel: bool) {
        let mutation = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        self.identity_changed_locked(grant, id, cancel, &mutation);
    }

    pub(crate) fn identity_changed_locked(
        &self,
        grant: i64,
        id: &RootId,
        cancel: bool,
        _mutation: &std::sync::MutexGuard<'_, ()>,
    ) {
        let changed = self
            .catalog
            .writer()
            .call(WriteCommand::GrantIdentityChanged { grant_id: grant });
        if changed.is_err() {
            return;
        }
        let _ = self.copies_fence(grant);
        let row = db::root_for_grant(&self.db(), grant);
        if let Ok(Some(row)) = row {
            self.freshness.remove(row);
        }
        if cancel {
            let _ = self
                .engine
                .scan_cancel_root(&loomward_engine::scan::RootKey::Grant(grant), id);
        }
        let _ = self
            .engine
            .scan_writer_fence(&loomward_engine::scan::RootKey::Grant(grant), id);
        // This callback has no reply channel; model reads retry cleanup and fail if it still fails.
        if self.engine.learning_redact_revoked().is_err() {
            eprintln!(
                "learning redaction failed after root identity changed; advice reads will retry"
            );
        }
        if let (Ok(revs), Ok(list)) = (self.revisions(), self.root_list(None)) {
            self.publish(EventName::RootsChanged, list, Some(revs.0), Some(revs.1));
        }
    }

    fn grants_list(&self) -> Handled {
        let revs = self.revisions()?;
        let rows = db::grant_rows(&self.db()).map_err(sql_error)?;
        let grants: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "grant_id": format!("gr_{}", r.grant),
                    "kind": "metadata_root",
                    "root_id": self.root_id(r.root),
                    "granted_at": ts_ns(r.granted_at_ns),
                    "granted_via": r.granted_via,
                    "revoked_at": r.revoked_at_ns.map(ts_ns),
                    "forgotten_at": null,
                })
            })
            .collect();
        // What is left of a forgotten root: dates and an ID, no path, name or volume.
        let forgotten =
            db::forgotten_grants(&self.db(), 256 - rows.len() as i64).map_err(sql_error)?;
        let grants = grants.into_iter().chain(forgotten.iter().map(|f| {
            json!({
                "grant_id": format!("gr_{}", f.grant),
                "kind": "metadata_root",
                "root_id": self.root_id(f.root),
                "granted_at": ts_ns(f.granted_at_ns),
                "granted_via": f.granted_via,
                "revoked_at": f.revoked_at_ns.map(ts_ns),
                "forgotten_at": ts_ns(f.forgotten_at_ns),
            })
        }));
        Ok(out(
            json!({"grants": grants.collect::<Vec<_>>()}),
            Some(revs),
        ))
    }

    /// One native dialog at a time (#167), and a 2 s cooldown between pickers, shared by the folder
    /// and file pickers: a second request is `busy`, never a stacked dialog.
    pub(crate) fn dialog_turn(&self) -> Result<std::sync::MutexGuard<'_, ()>, ErrorBody> {
        let Ok(turn) = self.dialog.try_lock() else {
            return Err(fail(ErrorCode::Busy, "a native dialog is already open"));
        };
        let mut last = self.picker_last.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(2)) {
            return Err(fail(
                ErrorCode::Busy,
                "native picker cooldown; retry shortly",
            ));
        }
        *last = Some(std::time::Instant::now());
        Ok(turn)
    }

    /// Desktop only. Personal sessions only (a synthetic session refuses before any dialog). The
    /// folder comes from the native picker, is checked exactly like a grant root (#167), and is
    /// granted under the same provenance rules. One dialog at a time.
    fn request_grant(&self, ctx: &CallContext) -> Handled {
        let refused = |r: RootGrantResultRefusal| {
            Ok(out(
                json!({"outcome": "refused", "root": null, "refusal": refusal_name(r)}),
                None,
            ))
        };
        if self.dataset == DatasetClass::Synthetic {
            return refused(RootGrantResultRefusal::SyntheticSessionRequiresLabRoot);
        }
        let dialogs = ctx.dialogs.as_ref().ok_or_else(|| {
            fail(
                ErrorCode::CapabilityUnavailable,
                "no native dialogs on this adapter",
            )
        })?;
        let _dialog = self.dialog_turn()?;
        let Some(picked) = dialogs.try_pick_folder()? else {
            return Ok(out(
                json!({"outcome": "cancelled_by_user", "root": null, "refusal": null}),
                None,
            ));
        };
        // Nested or overlapping roots are refused like a repeated grant. A stored grant that no
        // longer resolves still counts (one rule with `--static`, #224).
        let path = match paths::check_root(&picked, Some(&self.state_dir), &[], true) {
            Ok(p) => p,
            Err(r) => return refused(r),
        };
        if db::active_root_paths(&self.db())
            .map_err(sql_error)?
            .iter()
            .any(|stored| paths::overlaps_stored(&path, stored))
        {
            return refused(RootGrantResultRefusal::AlreadyGranted);
        }
        // #219: a volume that cannot verify file identity is granted only after the owner's
        // explicit native confirmation; declining is a cancellation, not a refusal.
        let filesystem = match self.root_filesystem(&path) {
            Ok(fs) => fs,
            Err(r) => return refused(r),
        };
        let unverified = !loomward_windows::enumerate::verifies_identity(&filesystem);
        if unverified && !dialogs.confirm_identity_unverified(&path, &filesystem) {
            return Ok(out(
                json!({"outcome": "cancelled_by_user", "root": null, "refusal": null}),
                None,
            ));
        }
        let (root, fresh) = match self.grant(&path, "desktop_picker", unverified) {
            Ok(g) => g,
            Err(crate::Refused::Rule(r)) => return refused(r),
            // Unreachable after confirmation; fail closed all the same.
            Err(crate::Refused::IdentityUnverified) => {
                return Ok(out(
                    json!({"outcome": "cancelled_by_user", "root": null, "refusal": null}),
                    None,
                ))
            }
            Err(crate::Refused::RootLimit) => {
                return Err(with_reason(
                    fail(
                        ErrorCode::ResourceBudget,
                        "64 roots are already granted; revoke one before granting another",
                    ),
                    "root_limit_reached",
                ))
            }
        };
        if !fresh {
            return refused(RootGrantResultRefusal::AlreadyGranted);
        }
        self.granted_root_reply(root)
    }

    /// The post-picker grant response and its root-list event use the same snapshot.
    pub(crate) fn granted_root_reply(&self, root: i64) -> Handled {
        // The picker is outside the mutation FIFO, and grant() released this lock before returning.
        // Keep the snapshot and its publication in one turn so a forget cannot fall between them.
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let revs = self.revisions()?;
        let rows = db::roots(&self.db(), Some(root)).map_err(sql_error)?;
        let row = rows
            .first()
            .ok_or_else(|| fail(ErrorCode::InternalError, "grant vanished"))?;
        let root_json = self.root_json(row, None)?;
        if let Ok(list) = self.root_list(None) {
            #[cfg(test)]
            self.hooks
                .gate(Command::RootsRequestGrant, "grant_snapshot");
            self.publish(EventName::RootsChanged, list, Some(revs.0), Some(revs.1));
        }
        Ok(out(
            json!({"outcome": "granted", "root": root_json, "refusal": null}),
            Some(revs),
        ))
    }

    fn volumes(&self, only: Option<i64>) -> Result<Vec<Value>, ErrorBody> {
        let rows = db::volumes(&self.db(), only).map_err(sql_error)?;
        // db::volumes reads one row past the bound so volumes.list can refuse; a change notice keeps 64.
        Ok(rows
            .iter()
            .take(64)
            .map(|v| {
                let declared = v.declared.flatten();
                json!({
                    "volume_id": self.volume_id(v.id),
                    "display_name": v.display_name.chars().take(64).collect::<String>(),
                    "mount_points": [],
                    "filesystem": v.filesystem.as_ref().map(|f| f.chars().take(32).collect::<String>()),
                    "label": null,
                    "online": v.online,
                    "read_only": null,
                    "removable": null,
                    "capacity_bytes": null,
                    "free_bytes": null,
                    "device": {"bus_type": "unknown", "seek_penalty": null, "multiple_disks": null, "basis": "unavailable"},
                    "tier": {
                        "tier": declared,
                        "basis": if declared.is_some() { "declared" } else { "unknown" },
                        "declared_tier": declared,
                        "hint_tier": null,
                        "note": if declared.is_some() { "Declared by the owner." } else { "No declaration and no device hint yet." },
                    },
                    "features": {"file_ids_128": null, "hard_links": null, "sparse_files": null, "compression": null, "reparse_points": null, "usn_journal": null},
                    "observed_at": ts_ns(v.observed_at_ns),
                })
            })
            .collect())
    }

    /// Append-only owner preference under the optional `expected_state_rev` precondition
    /// (section 3 step 3): a mismatch writes nothing and is `stale_generation` /
    /// `state_rev_changed`.
    fn declare_tier(&self, r: DeclareTierRequest, expected: Option<Generation>) -> Handled {
        let volume = self.volume_row(&r.volume_id)?;
        let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(test)]
        self.hooks.gate(Command::VolumesDeclareTier, "locked");
        let key = db::volumes(&self.db(), Some(volume))
            .map_err(sql_error)?
            .first()
            .map(|v| v.key.clone())
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?;
        let at = now_ns();
        let written = db::declare_tier(
            &mut self.db(),
            &key,
            r.tier.map(|t| t.get()),
            expected.map(|g| g.get() as i64),
            at,
        )
        .map_err(sql_error)?;
        let state_rev = written.map_err(|_| {
            stale(
                "state.db changed since your read; re-read and ask the owner again",
                "state_rev_changed",
            )
        })?;
        let catalog_rev = self.revisions()?.0;
        let volume_json = self
            .volumes(Some(volume))?
            .pop()
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?;
        let all = self.volumes(None)?;
        self.publish(
            EventName::VolumesChanged,
            json!({"volumes": all}),
            None,
            Some(state_rev),
        );
        Ok(Out {
            result: json!({"volume": volume_json, "preference_recorded_at": ts_ns(at)}),
            catalog_rev: Some(catalog_rev),
            state_rev: Some(state_rev),
        })
    }

    /// Bind section 5's precondition; the reader checks it inside the result's transaction.
    fn check_generation(
        &self,
        expected: Option<Generation>,
        key: NodeKey,
        reader: &mut Reader,
    ) -> Result<(), ErrorBody> {
        let Some(expected) = expected else {
            return Ok(());
        };
        if !matches!(key, NodeKey::Dir(_) | NodeKey::File(_)) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "expected_generation needs an anchor inside one root",
            ));
        }
        reader.expect_generation(key, expected.get());
        #[cfg(test)]
        if let Some(before_read) = self.hooks.before_generation_read.lock().unwrap().take() {
            before_read();
        }
        Ok(())
    }

    fn sealed(&self, value: Value, revs: (i64, i64)) -> Handled {
        let mut value = value;
        self.seal(&mut value)?;
        Ok(out(value, Some(revs)))
    }

    pub(crate) fn node_read(
        &self,
        req: &RequestEnvelope,
        work: Option<&crate::ReadWork>,
        read: impl FnOnce(&mut Reader, NodeKey) -> loomward_catalog::Result<Result<Value, ErrorBody>>,
    ) -> Handled {
        let r: NodeRefRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let mut reader = self.read_reader(work)?;
        let key = self.node(&r.node_id, &mut reader)?;
        if !matches!(key, NodeKey::Dir(_) | NodeKey::File(_)) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "this command needs a directory or file node",
            ));
        }
        self.check_generation(req.expected_generation, key, &mut reader)?;
        let value = read(&mut reader, key).map_err(catalog_error)??;
        self.sealed(value, revs)
    }

    fn tree_slice(&self, req: &RequestEnvelope, work: Option<&crate::ReadWork>) -> Handled {
        let r: TreeSliceRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let mut reader = self.read_reader(work)?;
        let anchor = match &r.anchor {
            Anchor::Atlas {} => NodeKey::Atlas,
            Anchor::Root { root_id } => {
                let dir = db::active_root_dir(&self.db(), self.root_row(root_id)?)
                    .map_err(sql_error)?
                    .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?;
                NodeKey::Dir(dir)
            }
            Anchor::Node { node_id } => self.node(node_id, &mut reader)?,
        };
        if !matches!(r.anchor, Anchor::Atlas {}) {
            self.check_generation(req.expected_generation, anchor, &mut reader)?;
        }
        let slice = reader
            .slice(&SliceRequest {
                anchor,
                depth: r.depth.get() as usize,
                max_nodes: r.max_nodes.get() as usize,
                min_share: r.min_share.get(),
                basis: basis(r.basis),
                include_files: r.include_files,
            })
            .map_err(catalog_error)?;
        self.sealed(encode(&slice)?, revs)
    }

    fn tree_children(&self, req: &RequestEnvelope, work: Option<&crate::ReadWork>) -> Handled {
        let r: TreeChildrenRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let mut reader = self.read_reader(work)?;
        let key = self.node(&r.node_id, &mut reader)?;
        let NodeKey::Dir(dir) = key else {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "tree.children needs a directory node",
            ));
        };
        self.check_generation(req.expected_generation, key, &mut reader)?;
        let bind = binding(req);
        let cursor = match &r.cursor {
            None => None,
            Some(handle) => match self
                .cursors
                .resume(handle, Command::TreeChildren, &bind, None)
            {
                Ok(Position::Children(c)) => Some(c),
                Ok(_) | Err(Resume::Mismatch) => {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        "the cursor belongs to another request",
                    ))
                }
                Err(_) => {
                    return Err(stale(
                        "the cursor expired; restart from the first page",
                        "cursor_expired",
                    ))
                }
            },
        };
        let page = reader
            .children(&ChildrenRequest {
                dir_id: dir,
                sort: match r.sort {
                    TreeChildrenRequestSort::SizeDesc => Sort::SizeDesc,
                    TreeChildrenRequestSort::NameAsc => Sort::NameAsc,
                    TreeChildrenRequestSort::ModifiedDesc => Sort::ModifiedDesc,
                },
                basis: basis(r.basis),
                limit: r.limit.get() as usize,
                cursor,
            })
            .map_err(catalog_error)?;
        let next = page.next_cursor.map(|c| {
            self.cursors
                .issue(Command::TreeChildren, bind, Position::Children(c), None)
        });
        let value = json!({
            "anchor": page.anchor,
            "generation": page.generation,
            "items": encode(&page.items)?,
            "next_cursor": next,
            "total": page.total,
            "budget_hit": page.budget_hit,
        });
        self.sealed(value, revs)
    }

    /// `search.query`; with `client`, the grant-scoped adapter (LW-069): its roots bound the
    /// rows, root states and generation, and the cursor binds the client and its grant epoch.
    pub(crate) fn search(
        &self,
        req: &RequestEnvelope,
        work: Option<&crate::ReadWork>,
        client: Option<&crate::clients::ClientGrant>,
    ) -> Handled {
        let r: SearchRequest = req.decode_payload()?;
        if r.uses_federated_fields() {
            return Err(fail(
                ErrorCode::CapabilityUnavailable,
                "federated search fields are not served by this engine (features.federated_search is false)",
            ));
        }
        if let Some(g) = client {
            g.check_filters(&r)?;
        }
        let revs = self.revisions()?;
        let root = r.root_id.as_ref().map(|id| self.root_row(id)).transpose()?;
        if let (Some(g), Some(row)) = (client, root) {
            if !g.roots.contains(&row) {
                // As an unknown root: nothing says the root exists.
                return Err(fail(ErrorCode::NotFound, "unknown root"));
            }
        }
        let bind = match client {
            Some(g) => format!("client:{}:{}:{}", g.client, g.epoch, binding(req)),
            None => binding(req),
        };
        let cursor = match &r.cursor {
            None => None,
            Some(handle) => match self
                .cursors
                .resume(handle, Command::SearchQuery, &bind, None)
            {
                Ok(Position::Search(c)) => Some(c),
                Ok(_) | Err(Resume::Mismatch) => {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        "the cursor belongs to another request",
                    ))
                }
                Err(_) => {
                    return Err(stale(
                        "the cursor expired; restart from the first page",
                        "cursor_expired",
                    ))
                }
            },
        };
        let mut reader = self.read_reader(work)?;
        let page = reader
            .search(&loomward_catalog::SearchRequest {
                root_id: root,
                text: r.text.to_string(),
                extension: r.extension.as_ref().map(|e| e.to_string()),
                min_bytes: r.min_bytes.map(|b| b.get()),
                max_bytes: r.max_bytes.map(|b| b.get()),
                modified_after: r.modified_after.as_ref().map(ft).transpose()?,
                modified_before: r.modified_before.as_ref().map(ft).transpose()?,
                kind: match r.kind {
                    SearchRequestKind::Any => "any",
                    SearchRequestKind::File => "file",
                    SearchRequestKind::Dir => "dir",
                }
                .into(),
                limit: r.limit.get() as usize,
                cursor,
                // SQLite VM steps, additionally interrupted at the request deadline.
                work_budget: 50_000_000,
                // P8: a page answers within about 300 ms, partial with budget_hit and a cursor if need be.
                time_budget: Some(std::time::Duration::from_millis(250)),
                scope: client.map(|g| g.roots.clone()),
            })
            .map_err(catalog_error)?;
        #[cfg(test)]
        self.hooks
            .gate(Command::SearchQuery, "search_page_captured");
        let next = page.next_cursor.map(|c| {
            self.cursors
                .issue(Command::SearchQuery, bind, Position::Search(c), None)
        });
        let mut freshness = std::collections::BTreeMap::new();
        for hit in &page.hits {
            freshness
                .entry(hit.root_id)
                .or_insert_with(|| self.hit_freshness(hit.root_id));
        }
        let mut stale_roots = std::collections::BTreeSet::new();
        for r in &page.roots {
            if r.scan_state == "never_scanned" || !self.freshness.stale(r.root_id) {
                continue;
            }
            // Before the first publication there is no old index for the watch to mark stale.
            if r.scan_state == "scanning" && !r.has_completed_scan {
                continue;
            }
            stale_roots.insert(r.root_id);
        }
        let stale_scope = !stale_roots.is_empty();
        let hits = page
            .hits
            .iter()
            .map(|h| {
                Ok(json!({
                    "row": encode(&h.row)?,
                    "root_id": self.root_id(h.root_id),
                    "path": encode(&h.path)?,
                    "match_kind": h.match_kind,
                    "freshness": freshness[&h.root_id],
                    "highlights": h.highlights,
                }))
            })
            .collect::<Result<Vec<_>, ErrorBody>>()?;
        // As roots.list: a root mid-repair reads partial.
        let roots: Vec<Value> = page
            .roots
            .iter()
            .map(|r| {
                json!({
                    "root_id": self.root_id(r.root_id),
                    "scan_state": if stale_roots.contains(&r.root_id) { "stale" } else if r.scan_state == "repairing" { "partial" } else { r.scan_state.as_str() },
                    "coverage": if stale_roots.contains(&r.root_id) { "stale" } else { r.coverage.as_str() },
                })
            })
            .collect();
        let value = json!({
            "generation": page.generation,
            "hits": hits,
            "next_cursor": next,
            "budget_hit": page.budget_hit,
            "index_state": if stale_scope { "stale" } else { page.index_state.as_str() },
            "partial": page.partial || stale_scope,
            "substring_searched": page.substring_searched,
            "roots": roots,
        });
        self.sealed(value, revs)
    }

    fn breakdown(&self, req: &RequestEnvelope, work: Option<&crate::ReadWork>) -> Handled {
        let r: BreakdownRequest = req.decode_payload()?;
        let revs = self.revisions()?;
        let mut reader = self.read_reader(work)?;
        let key = self.node(&r.node_id, &mut reader)?;
        let NodeKey::Dir(dir) = key else {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "stats.breakdown needs a directory node",
            ));
        };
        self.check_generation(req.expected_generation, key, &mut reader)?;
        let by = match r.by {
            BreakdownRequestBy::ExtFamily => "ext_family",
            BreakdownRequestBy::Extension => "extension",
            BreakdownRequestBy::AgeBand => "age_band",
        };
        let b = reader
            .breakdown(dir, by, basis(r.basis), r.limit.get() as usize, 50_000_000)
            .map_err(catalog_error)?;
        self.sealed(encode(&b)?, revs)
    }

    pub(crate) fn read_reader(&self, work: Option<&crate::ReadWork>) -> Result<Reader, ErrorBody> {
        #[cfg(test)]
        if let (Some(work), Some(now)) = (work, *self.hooks.read_clock.lock().unwrap()) {
            let reader = self.catalog.reader().map_err(catalog_error)?;
            let mut handle = work.interrupt.lock().unwrap();
            if now >= work.deadline {
                return Err(fail(
                    ErrorCode::DeadlineExceeded,
                    "the read deadline expired",
                ));
            }
            // SQLite uses real Instants; the injected receiver clock owns its hard stop instead.
            *handle = Some(reader.interrupt_handle());
            return Ok(reader);
        }
        match work {
            Some(work) => work.reader(&self.catalog),
            None => self.catalog.reader().map_err(catalog_error),
        }
    }
}

#[cfg(test)]
mod scrub_copy_tests {

    #[test]
    fn backup_byte_check_fails_closed_when_file_cannot_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        assert!(super::holds(
            &dir.path().join("missing.db"),
            "SyntheticPath"
        ));
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let path = dir.path().join("locked.db");
            std::fs::write(&path, b"unreadable contents").unwrap();
            let _lock = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&path)
                .unwrap();
            assert!(super::holds(&path, "SyntheticPath"));
        }
    }

    #[test]
    fn scrub_copy_checkpoints_wal_and_leaves_no_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state.db");
        let copy = dir.path().join("state.before-v6.db");
        rusqlite::Connection::open(&state).unwrap().execute_batch(
            "CREATE TABLE object_ref(id INTEGER PRIMARY KEY); CREATE TABLE root_forgotten(grant_id INTEGER); INSERT INTO root_forgotten VALUES(1)"
        ).unwrap();
        let conn = rusqlite::Connection::open(&copy).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE object_ref(id INTEGER PRIMARY KEY); CREATE TABLE root_grant(id INTEGER PRIMARY KEY,display_path TEXT,volume_key TEXT,root_file_id BLOB); PRAGMA user_version=6; INSERT INTO root_grant VALUES(1,'SyntheticForgottenPath','fixture',zeroblob(8)),(2,'SyntheticKeptPath','fixture',zeroblob(8))").unwrap();
        // Simulate a copied/crashed WAL database, preserving its committed WAL frames.
        let saved = dir.path().join("saved.db");
        for suffix in ["", "-wal", "-shm"] {
            std::fs::copy(
                format!("{}{suffix}", copy.display()),
                format!("{}{suffix}", saved.display()),
            )
            .unwrap();
        }
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            std::fs::rename(
                format!("{}{suffix}", saved.display()),
                format!("{}{suffix}", copy.display()),
            )
            .unwrap();
        }
        let (known, _pin) = super::scrub_copy(&copy, &state).unwrap();
        assert!(known);
        let copy_conn = rusqlite::Connection::open(&copy).unwrap();
        let mode: String = copy_conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "delete", "scrubbing must not write a new WAL");
        assert_eq!(
            copy_conn
                .query_row("SELECT display_path FROM root_grant WHERE id=2", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            "SyntheticKeptPath"
        );
        drop(copy_conn);
        assert!(!super::holds(&copy, "SyntheticForgottenPath"));
        for suffix in ["-wal", "-shm"] {
            assert!(!std::path::Path::new(&format!("{}{suffix}", copy.display())).exists());
        }
    }

    #[cfg(windows)]
    #[test]
    fn backup_pin_accepts_owned_files_and_refuses_aliases_without_an_elevation_gate() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("synthetic-copy.db");
        std::fs::write(&copy, b"synthetic backup").unwrap();
        let pin = super::pin_backup(&copy).unwrap();
        assert!(
            std::fs::remove_file(&copy).is_err(),
            "pin must deny deletion"
        );
        assert!(std::fs::rename(&copy, dir.path().join("replacement.db")).is_err());
        drop(pin);

        let alias = dir.path().join("synthetic-alias.db");
        std::fs::hard_link(&copy, &alias).unwrap();
        assert!(super::pin_backup(&copy).is_err());
        assert!(super::pin_backup(&alias).is_err());
        std::fs::remove_file(&alias).unwrap();
        drop(super::pin_backup(&copy).unwrap());

        let directory = dir.path().join("synthetic-directory");
        std::fs::create_dir(&directory).unwrap();
        assert!(super::pin_backup(&directory).is_err());
        let junction = dir.path().join("synthetic-junction");
        let made = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&directory)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(super::pin_backup(&junction).is_err());
        std::fs::remove_dir(&junction).unwrap();
        std::fs::remove_file(&copy).unwrap();
    }

    /// Explicit resource probe, kept out of the ordinary suite's I/O budget.
    #[test]
    #[ignore = "manual 1 GiB streaming-memory probe; run explicitly with --ignored --nocapture"]
    #[cfg(windows)]
    fn large_sparse_backup_memory_probe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic-sparse-copy.db");
        let file = std::fs::File::create(&path).unwrap();
        assert!(std::process::Command::new("fsutil.exe")
            .args(["sparse", "setflag"])
            .arg(&path)
            .status()
            .unwrap()
            .success());
        file.set_len(1024 * 1024 * 1024).unwrap();
        drop(file);
        println!("ready");
        let started = std::time::Instant::now();
        assert!(!super::holds(&path, "SyntheticAbsentMarker"));
        println!(
            "{}",
            serde_json::json!({"sparse_bytes": 1073741824u64, "scan_seconds": started.elapsed().as_secs_f64()})
        );
    }

    #[test]
    fn streaming_backup_check_preserves_split_matches_and_read_failures() {
        use std::io::{self, Read};
        let needle = "Synthetic ?? name";
        let utf16: Vec<_> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
        for pattern in [needle.as_bytes(), utf16.as_slice()] {
            for prefix in [65533, 65535, 65536] {
                let mut bytes = vec![0; prefix];
                bytes.extend_from_slice(pattern);
                assert!(super::holds_reader(bytes.as_slice(), needle));
            }
        }
        assert!(!super::holds_reader(&b"different"[..], needle));
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic mid-stream failure"))
            }
        }
        assert!(super::holds_reader(
            io::repeat(0).take(65537).chain(Broken),
            needle
        ));
        struct Bounded(u64);
        impl Read for Bounded {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                assert!(bytes.len() <= 65536, "unbounded backup read");
                let n = bytes.len().min(self.0 as usize);
                bytes[..n].fill(0);
                self.0 -= n as u64;
                Ok(n)
            }
        }
        assert!(!super::holds_reader(Bounded(2 * 1024 * 1024), needle));
    }
    use super::scrub_copy;

    /// A table name is an identifier, never SQL: a copy table named with a quote and a statement
    /// is scrubbed like any other keyed table, and the statement it spells never runs.
    #[test]
    fn scrub_copy_treats_table_names_as_identifiers_not_sql() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state.db");
        let copy = dir.path().join("state.before-v7.db");
        rusqlite::Connection::open(&state)
            .unwrap()
            .execute_batch("CREATE TABLE object_ref (id INTEGER PRIMARY KEY); INSERT INTO object_ref VALUES (1);")
            .unwrap();
        let evil = "evil\"; DROP TABLE sentinel; --";
        let quoted = format!("\"{}\"", evil.replace('"', "\"\""));
        rusqlite::Connection::open(&copy)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE object_ref (id INTEGER PRIMARY KEY); INSERT INTO object_ref VALUES (1),(2);
                 CREATE TABLE sentinel (k INTEGER); INSERT INTO sentinel VALUES (7);
                 CREATE TABLE {quoted} (object_ref_id INTEGER); INSERT INTO {quoted} VALUES (1),(2);"
            ))
            .unwrap();
        scrub_copy(&copy, &state).unwrap();
        let old = rusqlite::Connection::open(&copy).unwrap();
        let count = |sql: &str| old.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT count(*) FROM sentinel"), 1);
        assert_eq!(count(&format!("SELECT count(*) FROM {quoted}")), 1);
    }
}
