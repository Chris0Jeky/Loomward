//! Bounded native scan workers, staging, run/revision finals and grant revocation hooks.
mod pipeline;
pub mod sink;
pub mod source;
#[cfg(test)]
mod tests;
#[cfg(any(windows, test))]
pub mod watch;
use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{DatasetClass, Job, JobId, JobKind, RootId, ScanBudget};
pub use pipeline::{run_scan, ScanOptions, ScanReport};
pub use sink::{Entry, ListingTicket, MemorySink, RunScope, ScanMessage, ScanSink, Sums};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

#[derive(Default)]
pub(crate) struct RootGate {
    revoked: bool,
    duplicate_cancel: Arc<AtomicBool>,
    aliases: std::collections::HashSet<RootId>,
}

/// Stable gate identity; wire IDs remain labels for jobs and sink messages only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RootKey {
    /// Primary key in this engine's durable grant store.
    Grant(i64),
    /// Standalone lab and test roots have no durable grant row.
    Fixture(RootId),
}

/// Catalogue identity resolved by the service after authenticating its wire root ID.
#[derive(Debug)]
pub struct CatalogRootIdentity {
    /// Primary key of the catalogue root row.
    pub row: i64,
    /// Full catalogue instance ID, not a truncated wire tag.
    pub instance: String,
}

/// Trusted root resolved from an active durable grant by the service, never from wire paths.
///
/// Callers cannot change the granted path or mint a root from their own identity observation.
/// ```compile_fail
/// fn replace_path(root: &mut loomward_engine::scan::GrantedRoot) {
///     root.path = "fixture".into();
/// }
/// ```
/// ```compile_fail
/// use loomward_engine::scan::{GrantedRoot, source::OpenedIdentity};
/// use loomward_protocol::{DatasetClass, RootId};
/// fn mint(identity: OpenedIdentity) {
///     GrantedRoot::new(RootId::new("rt_fixture").unwrap(), "fixture".into(), DatasetClass::Synthetic, identity);
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantedRoot {
    root_id: RootId,
    key: RootKey,
    path: PathBuf,
    dataset_class: DatasetClass,
    pub(crate) expected: Option<source::OpenedIdentity>,
}
impl GrantedRoot {
    // Minted by `Engine::granted_root` from a durable grant row, and by fixtures.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn new(
        root_id: RootId,
        path: PathBuf,
        dataset_class: DatasetClass,
        expected: source::OpenedIdentity,
    ) -> Self {
        Self {
            key: RootKey::Fixture(root_id.clone()),
            root_id,
            path,
            dataset_class,
            expected: Some(expected),
        }
    }
    /// Construct a synthetic grant only for a marked G:/E: scale-lab data directory.
    /// This fixture-only API is absent from production builds; L8 resolves personal grants.
    #[cfg(all(windows, feature = "lab"))]
    pub fn for_lab(root_id: RootId, path: PathBuf) -> EngineResult<Self> {
        use source::DirSource;
        let text = path.to_str().ok_or_else(|| EngineError::PermissionDenied {
            message: "invalid lab root".into(),
        })?;
        let normal = text.strip_prefix(r"\\?\").unwrap_or(text);
        let prefix = [r"G:\loomward-lab\scale\", r"E:\loomward-lab\scale\"]
            .into_iter()
            .find(|p| normal.starts_with(p))
            .ok_or_else(|| EngineError::PermissionDenied {
                message: "outside scale lab".into(),
            })?;
        let suffix = &normal[prefix.len()..];
        let name = suffix
            .strip_suffix(r"\data")
            .ok_or_else(|| EngineError::PermissionDenied {
                message: "not lab data root".into(),
            })?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(EngineError::PermissionDenied {
                message: "invalid lab name".into(),
            });
        }
        let source = loomward_windows::enumerate::NativeSource::default();
        let mut pins = Vec::new();
        for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            pins.push(
                source
                    .open_root(ancestor)
                    .map_err(|_| EngineError::PermissionDenied {
                        message: "lab ancestor refused".into(),
                    })?
                    .0,
            );
        }
        let marker = path.parent().unwrap().join(".loomward-lab-marker");
        if std::fs::read_to_string(marker).ok().as_deref()
            != Some("Loomward disposable scale lab v1\n")
        {
            return Err(EngineError::PermissionDenied {
                message: "unmarked lab root".into(),
            });
        }
        let (_, identity) = source
            .open_root(&path)
            .map_err(|_| EngineError::PermissionDenied {
                message: "lab root refused".into(),
            })?;
        Ok(Self::new(root_id, path, DatasetClass::Synthetic, identity))
    }
    /// Opaque grant root id.
    pub fn root_id(&self) -> &RootId {
        &self.root_id
    }
    /// Stable identity used for start, revocation and writer fencing.
    pub fn key(&self) -> &RootKey {
        &self.key
    }
    /// Granted native directory; never exposed as a wire input.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Dataset class of its durable grant.
    pub fn dataset_class(&self) -> DatasetClass {
        self.dataset_class
    }
}
impl Engine {
    /// Content equality evidence under a freshly checked active synthetic grant.
    /// Personal grants remain metadata-only; root revocation requests content-read cancellation.
    /// Revocation can return before I/O drains; the final gate refuses a revoked report.
    pub fn inspect_duplicates(
        &self,
        root: &GrantedRoot,
        entries: &[loomward_windows::duplicate_reader::Entry],
        budget: loomward_windows::duplicate_reader::Budget,
    ) -> EngineResult<loomward_windows::duplicate_reader::Report> {
        self.inspect_duplicates_cancel(root, entries, budget, None)
    }
    pub(crate) fn inspect_duplicates_cancel(
        &self,
        root: &GrantedRoot,
        entries: &[loomward_windows::duplicate_reader::Entry],
        budget: loomward_windows::duplicate_reader::Budget,
        job_cancel: Option<Arc<AtomicBool>>,
    ) -> EngineResult<loomward_windows::duplicate_reader::Report> {
        let root_cancel = if job_cancel.is_some() {
            self.duplicate_read_admission_worker(root)?
        } else {
            self.duplicate_read_admission(root)?
        };
        let cancel = job_cancel.unwrap_or_else(|| root_cancel.clone());
        let report = loomward_windows::duplicate_reader::inspect(
            &root.path,
            root.expected.as_ref().unwrap(),
            entries,
            budget,
            cancel.clone(),
        )
        .map_err(|reason| EngineError::PermissionDenied {
            message: reason.as_str().into(),
        })?;
        // Refuse evidence if revocation is observed at this final gate.
        let gate = self.scan_root_gate(root.key());
        let revoked = gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if revoked.revoked || root_cancel.load(Ordering::Acquire) {
            return Err(EngineError::PermissionDenied {
                message: "cancelled".into(),
            });
        }
        Ok(report)
    }
    pub(crate) fn duplicate_read_admission(
        &self,
        root: &GrantedRoot,
    ) -> EngineResult<Arc<AtomicBool>> {
        self.duplicate_read_admission_inner(root, false)
    }
    pub(crate) fn duplicate_read_admission_worker(
        &self,
        root: &GrantedRoot,
    ) -> EngineResult<Arc<AtomicBool>> {
        self.duplicate_read_admission_inner(root, true)
    }
    fn duplicate_read_admission_inner(
        &self,
        root: &GrantedRoot,
        wait: bool,
    ) -> EngineResult<Arc<AtomicBool>> {
        use rusqlite::OpenFlags;
        let denied = |message: &str| EngineError::PermissionDenied {
            message: message.into(),
        };
        self.check_root(root)?;
        if root.dataset_class != DatasetClass::Synthetic {
            return Err(denied("content_read_gated"));
        }
        let gate = self.scan_root_gate(root.key());
        let revoked = if wait {
            // Workers wait for the brief start/revocation fence; callers keep immediate busy.
            gate.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        } else {
            match gate.try_lock() {
                Ok(guard) => guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    return Err(EngineError::Busy {
                        message: "root already starting or revoking".into(),
                        detail: None,
                    })
                }
            }
        };
        if revoked.revoked {
            return Err(denied("root revoked"));
        }
        let expected = root
            .expected
            .as_ref()
            .ok_or_else(|| denied("unsupported"))?;
        let id = match expected.id {
            Some(source::FileIdObs::Id128(id)) => id.to_vec(),
            Some(source::FileIdObs::Id64(id)) => id.to_le_bytes().to_vec(),
            None => return Err(denied("unsupported")),
        };
        let volume = expected
            .volume_serial
            .ok_or_else(|| denied("unsupported"))?;
        let conn = rusqlite::Connection::open_with_flags(
            self.config.state_dir.join("state.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| denied("grant store unavailable"))?;
        // GrantedRoot carries native identity, not a durable row id: ambiguous matches fail closed.
        let (count, active, class): (i64, i64, String) = conn
            .query_row(
                "SELECT count(*),count(CASE WHEN state='active' THEN 1 END),
                    (SELECT value FROM meta WHERE key='dataset_class')
             FROM root_grant WHERE display_path=?1 AND volume_key=?2 AND root_file_id=?3",
                rusqlite::params![
                    root.path.to_str().ok_or_else(|| denied("invalid_path"))?,
                    format!("vsn_{volume}"),
                    id
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| denied("grant store unavailable"))?;
        if class != "synthetic" {
            return Err(denied("dataset_class_mismatch"));
        }
        if count != 1 || active != 1 {
            return Err(denied("grant_not_active"));
        }
        Ok(revoked.duplicate_cancel.clone())
    }
    /// The only production way to mint a [`GrantedRoot`] (#144): read grant `grant_id` from this
    /// engine's own `state.db`, require it `active`, take the path and dataset class from that
    /// durable row (never from a caller), and require the opened native root to carry the
    /// grant's volume and file identity. The dataset class is reconciled at scan start.
    pub fn granted_root(
        &self,
        root_id: RootId,
        grant_id: i64,
        catalog_root: &CatalogRootIdentity,
    ) -> EngineResult<GrantedRoot> {
        use rusqlite::{OpenFlags, OptionalExtension};
        let denied = |message: &str| EngineError::PermissionDenied {
            message: message.into(),
        };
        let conn = rusqlite::Connection::open_with_flags(
            self.config.state_dir.join("state.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| denied("grant store unavailable"))?;
        let row: Option<(String, String, Vec<u8>, String)> = conn
            .query_row(
                "SELECT g.display_path,g.volume_key,g.root_file_id,
                        (SELECT value FROM meta WHERE key='dataset_class')
                 FROM root_grant g WHERE g.id=?1 AND g.state='active'",
                [grant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|_| denied("grant store unavailable"))?;
        let (path, volume_key, file_id, class) = row.ok_or_else(|| denied("grant_not_active"))?;
        // Wire IDs are opaque here; the authenticated identity must name this grant's root.
        let paired = rusqlite::Connection::open_with_flags(
            self.config.state_dir.join("catalog.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .and_then(|c| {
            c.query_row(
                "SELECT (SELECT value FROM meta WHERE key='instance_id')
                 FROM root WHERE id=?1 AND grant_id=?2",
                rusqlite::params![catalog_root.row, grant_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .map_err(|_| denied("grant store unavailable"))?
        .is_some_and(|instance| instance == catalog_root.instance);
        if !paired {
            return Err(denied("grant_root_mismatch"));
        }
        let dataset_class = match class.as_str() {
            "synthetic" => DatasetClass::Synthetic,
            "personal" => DatasetClass::Personal,
            _ => return Err(denied("dataset_class_mismatch")),
        };
        #[cfg(windows)]
        {
            use source::{DirSource, FileIdObs};
            let path = PathBuf::from(path);
            let (_, identity) = loomward_windows::enumerate::NativeSource::default()
                .open_root(&path)
                .map_err(|_| denied("root open refused"))?;
            let same_id = match identity.id {
                Some(FileIdObs::Id128(id)) => file_id == id,
                Some(FileIdObs::Id64(id)) => file_id == id.to_le_bytes(),
                None => false,
            };
            if !same_id || identity.volume_serial.map(|s| format!("vsn_{s}")) != Some(volume_key) {
                return Err(denied("root_identity_changed"));
            }
            let mut root = GrantedRoot::new(root_id, path, dataset_class, identity);
            root.key = RootKey::Grant(grant_id);
            self.scan_root_gate(root.key())
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .aliases
                .insert(root.root_id().clone());
            Ok(root)
        }
        #[cfg(not(windows))]
        {
            let _ = (root_id, path, volume_key, file_id, dataset_class);
            Err(EngineError::unavailable(Component::Scan))
        }
    }
    fn check_root(&self, root: &GrantedRoot) -> EngineResult<()> {
        if root.dataset_class != self.config.dataset_class {
            return Err(EngineError::PermissionDenied {
                message: "dataset_class_mismatch".into(),
            });
        }
        Ok(())
    }
    /// Install the catalogue writer after restart recovery has marked interrupted roots repairing.
    pub fn set_scan_sink(&self, sink: Arc<dyn ScanSink>) -> EngineResult<()> {
        let mut installed = self.scan_sink.lock().unwrap();
        if installed.is_some() {
            return Err(EngineError::Busy {
                message: "scan writer already installed".into(),
                detail: None,
            });
        }
        sink.recover_interrupted()?;
        *installed = Some(sink);
        Ok(())
    }
    /// Full read-only traversal of one granted root.
    pub fn scan_start(&self, root: &GrantedRoot, budget: Option<ScanBudget>) -> EngineResult<Job> {
        self.start_scan(root, budget, RunScope::FullRoot)
    }
    /// Refresh with a full baseline and reconcile hints collected during this scan only.
    pub fn scan_refresh(
        &self,
        root: &GrantedRoot,
        budget: Option<ScanBudget>,
    ) -> EngineResult<Job> {
        self.start_scan(root, budget, RunScope::FullRoot)
    }
    fn scan_root_gate(&self, root: &RootKey) -> Arc<std::sync::Mutex<RootGate>> {
        self.scan_roots
            .lock()
            .unwrap()
            .entry(root.clone())
            .or_insert_with(|| {
                let aliases = match root {
                    RootKey::Fixture(id) => std::iter::once(id.clone()).collect(),
                    RootKey::Grant(_) => Default::default(),
                };
                Arc::new(std::sync::Mutex::new(RootGate {
                    aliases,
                    ..Default::default()
                }))
            })
            .clone()
    }
    fn start_scan(
        &self,
        root: &GrantedRoot,
        budget: Option<ScanBudget>,
        scope: RunScope,
    ) -> EngineResult<Job> {
        self.check_root(root)?;
        let sink = self
            .scan_sink
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| EngineError::unavailable(Component::Scan))?;
        let gate = self.scan_root_gate(root.key());
        // Reserve before any probe/stop/start, and retain through job submission.
        // A panic elsewhere must not leave the root Busy forever: recover a poisoned gate.
        let mut revoked = match gate.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(EngineError::Busy {
                    message: "root already starting or revoking".into(),
                    detail: None,
                })
            }
        };
        if revoked.revoked {
            return Err(EngineError::PermissionDenied {
                message: "root revoked".into(),
            });
        }
        revoked.aliases.insert(root.root_id().clone());
        if let Some(job_id) = revoked
            .aliases
            .iter()
            .flat_map(|id| self.jobs.root_jobs(id))
            .next()
        {
            return Err(EngineError::Busy {
                message: "root already scanning".into(),
                detail: Some(
                    loomward_protocol::Detail::new(std::collections::BTreeMap::from([(
                        "job_id".into(),
                        serde_json::json!(job_id),
                    )]))
                    .unwrap(),
                ),
            });
        }
        let worker_permit = self.budgets.reserve_scan_workers(
            budget
                .as_ref()
                .and_then(|b| b.threads)
                .map(|n| n.get() as usize),
        )?;
        let options = ScanOptions {
            workers: worker_permit.count,
            max_entries: budget
                .as_ref()
                .and_then(|b| b.max_entries)
                .map_or(50_000_000, |n| n.get() as u64),
            max_dirs: budget
                .as_ref()
                .and_then(|b| b.max_dirs)
                .map_or(8_000_000, |n| n.get() as usize),
            scope,
            excluded_path: Some(self.config.state_dir.clone()),
            ..ScanOptions::default()
        };
        let root = root.clone();
        let bytes = self.budgets.bytes.clone();
        static NEXT_RUN: AtomicU64 = AtomicU64::new(1);
        let run = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
        // A sink that refuses watch hints (the trait default) is never watched: its scans are
        // plain full passes, and no watch pins outlive them (#184 MEDIUM-2).
        #[cfg(windows)]
        let watched = match sink.watch_dirty(root.root_id()) {
            Ok(()) => true,
            Err(EngineError::Unavailable { .. }) => false,
            Err(e) => return Err(e),
        };
        #[cfg(windows)]
        if !watched {
            let io = source::native_cancel();
            let interrupt = io.clone();
            let root_id = root.root_id().clone();
            let workers = std::sync::Mutex::new(Some(worker_permit));
            let runner: crate::jobs::JobRunner = Arc::new(move |ctx, _| {
                let _workers = workers
                    .lock()
                    .unwrap()
                    .take()
                    .expect("scan runner executes once");
                let source = loomward_windows::enumerate::NativeSource::new(io.clone());
                let report = run_scan(
                    &source,
                    &root,
                    run,
                    sink.clone(),
                    bytes.clone(),
                    ctx.cancel.clone(),
                    options.clone(),
                )?;
                ctx.set_scan_report(&report)?;
                Ok(if report.complete {
                    loomward_protocol::CoverageState::Complete
                } else {
                    loomward_protocol::CoverageState::Partial
                })
            });
            return self.jobs.submit(
                crate::jobs::JobSpec::new(JobKind::Scan, Some(root_id)),
                runner,
                self.events.clone(),
                Some(Arc::new(move || interrupt.cancel())),
            );
        }
        #[cfg(windows)]
        {
            let watch = {
                let existing = self
                    .scan_watches
                    .lock()
                    .unwrap()
                    .get(root.root_id())
                    .cloned();
                if let Some(watch) = existing {
                    watch.stop();
                }
                let watch = watch::RootWatch::start(&root, sink.clone())?;
                let replaced = self
                    .scan_watches
                    .lock()
                    .unwrap()
                    .insert(root.root_id().clone(), watch.clone());
                drop(replaced);
                watch
            };
            let io = source::native_cancel();
            let interrupt = io.clone();
            let root_id = root.root_id().clone();
            let workers = std::sync::Mutex::new(Some(worker_permit));
            let submit_watch = watch.clone();
            let runner: crate::jobs::JobRunner = Arc::new(move |ctx, _| {
                let _workers = workers
                    .lock()
                    .unwrap()
                    .take()
                    .expect("scan runner executes once");
                let source = loomward_windows::enumerate::NativeSource::new(io.clone());
                let report = watch::run_watched(
                    &source,
                    &root,
                    run,
                    sink.clone(),
                    bytes.clone(),
                    ctx.cancel.clone(),
                    options.clone(),
                    &watch,
                )?;
                ctx.set_scan_report(&report)?;
                Ok(if report.complete {
                    loomward_protocol::CoverageState::Complete
                } else {
                    loomward_protocol::CoverageState::Partial
                })
            });
            let job = self.jobs.submit(
                crate::jobs::JobSpec::new(JobKind::Scan, Some(root_id)),
                runner,
                self.events.clone(),
                Some(Arc::new(move || interrupt.cancel())),
            );
            if job.is_err() {
                submit_watch.stop();
            }
            job
        }
        #[cfg(not(windows))]
        {
            let _ = (options, root, bytes, run, sink);
            Err(EngineError::unavailable(Component::Scan))
        }
    }
    /// Immediate cancellation acknowledgement; no waiting for directory I/O or writer joins.
    pub fn scan_cancel(&self, id: &JobId) -> EngineResult<Job> {
        if self.job_status(id)?.kind != JobKind::Scan {
            return Err(EngineError::InvalidRequest {
                message: "not a scan job".into(),
            });
        }
        self.job_cancel(id)
    }
    /// Cancel active root jobs and redact retained duplicate evidence after durable revocation.
    pub fn scan_cancel_root(&self, root: &RootKey, wire_id: &RootId) -> EngineResult<Vec<Job>> {
        let gate = self.scan_root_gate(root);
        let mut revoked = gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        revoked.aliases.insert(wire_id.clone());
        revoked.revoked = true;
        revoked.duplicate_cancel.store(true, Ordering::Release);
        // Watch teardown can terminate a queued runner before it reaches the byte budget.
        let cancelled = revoked
            .aliases
            .iter()
            .flat_map(|id| self.jobs.root_jobs_for_revocation(id))
            .map(|id| self.jobs.cancel_revoked(&id, &self.events))
            .collect();
        #[cfg(windows)]
        {
            for id in &revoked.aliases {
                let watch = self.scan_watches.lock().unwrap().remove(id);
                if let Some(watch) = watch {
                    watch.stop();
                }
            }
        }
        cancelled
    }
    /// Writer-fence hook, called after durable revocation and root cancellation by L8.
    pub fn scan_writer_fence(&self, root: &RootKey, wire_id: &RootId) -> EngineResult<()> {
        let gate = self.scan_root_gate(root);
        let mut gate = gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gate.aliases.insert(wire_id.clone());
        let sink = self
            .scan_sink
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| EngineError::unavailable(Component::Scan))?;
        for id in &gate.aliases {
            sink.fence_root(id)?;
        }
        Ok(())
    }
}
