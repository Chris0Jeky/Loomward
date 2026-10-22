//! Bounded native scan workers, staging, run/revision finals and grant revocation hooks.
mod pipeline;
pub mod sink;
pub mod source;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tree_gate_tests;
#[cfg(any(windows, test))]
pub mod watch;
use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{DatasetClass, Job, JobId, JobKind, RootId, ScanBudget};
pub use pipeline::{run_scan, ScanOptions, ScanReport};
pub use sink::{
    Entry, ListingTicket, MemorySink, RunScope, ScanMessage, ScanSink, Sums, TargetDirectory,
};
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
    starting: Arc<AtomicBool>,
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
    /// #219: may list identity-unverified on FAT, exFAT or unknown filesystems. Synthetic and
    /// lab roots may; a personal root only when its grant record says so.
    allow_identity_unverified: bool,
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
            allow_identity_unverified: dataset_class == DatasetClass::Synthetic,
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
    /// Handle-derived identity for a grant-scoped, read-only watcher or journal open.
    pub fn expected_identity(&self) -> Option<source::OpenedIdentity> {
        self.expected
    }
    /// The identity the durable grant verified at admission (docs/47 C5 binds its observations to it).
    pub fn expected(&self) -> Option<&source::OpenedIdentity> {
        self.expected.as_ref()
    }
    /// Dataset class of its durable grant.
    pub fn dataset_class(&self) -> DatasetClass {
        self.dataset_class
    }
    /// Whether this root may list identity-unverified (#219); NTFS and ReFS never do.
    pub fn allows_identity_unverified(&self) -> bool {
        self.allow_identity_unverified
    }
}
/// The native source for a granted root, identity-unverified only when its grant allows it.
#[cfg(windows)]
fn native_source(
    root: &GrantedRoot,
    io: loomward_windows::enumerate::IoCancellation,
) -> loomward_windows::enumerate::NativeSource {
    let source = loomward_windows::enumerate::NativeSource::new(io);
    if root.allow_identity_unverified {
        source.with_identity_unverified()
    } else {
        source
    }
}
impl Engine {
    /// Read-only C1 evidence. Both roots must pass the existing q-10 admission gate.
    /// No service, HTTP, IPC or MCP command is exposed.
    pub fn compare_trees(
        &self,
        source: &GrantedRoot,
        destination: &GrantedRoot,
        budget: loomward_windows::duplicate_reader::Budget,
        max_entries: usize,
    ) -> EngineResult<loomward_operations::tree::Report> {
        use loomward_windows::duplicate_reader::{read_tree, Budget};
        use std::time::Instant;
        let left_cancel = self.duplicate_read_admission(source)?;
        let right_cancel = self.duplicate_read_admission(destination)?;
        let started = Instant::now();
        let mut remaining_bytes = budget.bytes;
        let mut read = |root: &GrantedRoot, cancel| {
            let (tree, bytes) = read_tree(
                root.path(),
                root.expected.as_ref().unwrap(),
                Budget {
                    bytes: remaining_bytes,
                    time: budget.time.saturating_sub(started.elapsed()),
                },
                max_entries,
                cancel,
            )
            .map_err(|reason| EngineError::PermissionDenied {
                message: reason.as_str().into(),
            })?;
            remaining_bytes = remaining_bytes.saturating_sub(bytes);
            Ok::<_, EngineError>(tree)
        };
        let left = read(source, left_cancel.clone())?;
        #[cfg(test)]
        tree_gate_tests::point(tree_gate_tests::TreePoint::AfterSourceRead);
        let right = read(destination, right_cancel.clone())?;
        #[cfg(test)]
        tree_gate_tests::point(tree_gate_tests::TreePoint::AfterDestinationRead);
        let report = loomward_operations::tree::compare(&left, &right);
        // Recheck both durable grants and cancellation after both observations, as in q-10.
        for (root, cancel) in [(source, left_cancel), (destination, right_cancel)] {
            self.duplicate_read_admission(root)?;
            if cancel.load(Ordering::Acquire) {
                return Err(EngineError::PermissionDenied {
                    message: "cancelled".into(),
                });
            }
        }
        #[cfg(test)]
        tree_gate_tests::point(tree_gate_tests::TreePoint::BeforeGuard);
        let mut gates = vec![
            self.scan_root_gate(source.key()),
            self.scan_root_gate(destination.key()),
        ];
        // The same root may appear twice; crossed pairs must take locks in the same order.
        gates.sort_by_key(Arc::as_ptr);
        gates.dedup_by(|a, b| Arc::ptr_eq(a, b));
        let guards: Vec<_> = gates
            .iter()
            .map(|gate| {
                gate.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            })
            .collect();
        if guards
            .iter()
            .any(|gate| gate.revoked || gate.duplicate_cancel.load(Ordering::Acquire))
        {
            return Err(EngineError::PermissionDenied {
                message: "cancelled".into(),
            });
        }
        Ok(report)
    }

    /// Content equality evidence under a freshly checked active grant of this engine's dataset
    /// class (personal reads opened by owner decision q-10); a session never reads the other
    /// class's roots. Root revocation requests content-read cancellation.
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
        let job_cancel_was_some = job_cancel.is_some();
        let root_cancel = if job_cancel_was_some {
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
        #[cfg(test)]
        if let Some(hook) = tests::DUPLICATE_READ_DONE_HOOK.with_borrow_mut(Option::take) {
            hook();
        }
        // A durable revoke can commit while the read ran, ahead of its in-memory fence.
        if job_cancel_was_some {
            self.duplicate_read_admission_worker(root)?;
        } else {
            self.duplicate_read_admission(root)?;
        }
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
        // The grant, the engine and (below) the durable store must all name one dataset class.
        self.check_root(root)?;
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
        // #219: rows of an identity-unverified root are never content-read inputs.
        if expected.basis == source::IdBasis::Unverified {
            return Err(denied("identity_unverified"));
        }
        let id = match expected.id {
            Some(source::FileIdObs::Id128(id)) => id.to_vec(),
            Some(source::FileIdObs::Id64(id)) => id.to_le_bytes().to_vec(),
            None => return Err(denied("unsupported")),
        };
        let volume = expected
            .volume_serial
            .ok_or_else(|| denied("unsupported"))?;
        let conn = rusqlite::Connection::open_with_flags(
            loomward_catalog::sqlite_dir(&self.config.state_dir).join("state.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| denied("grant store unavailable"))?;
        // GrantedRoot carries native identity, not a durable row id: ambiguous active matches fail
        // closed. Revoked rows for the same folder (a revoke, then a fresh grant) do not count.
        let (active, class): (i64, String) = conn
            .query_row(
                "SELECT count(CASE WHEN state='active' THEN 1 END),
                    (SELECT value FROM meta WHERE key='dataset_class')
             FROM root_grant WHERE display_path=?1 AND volume_key=?2 AND root_file_id=?3 AND volume_guid IS ?4",
                rusqlite::params![
                    root.path.to_str().ok_or_else(|| denied("invalid_path"))?,
                    format!("vsn_{volume}"),
                    id,
                    expected.volume_guid.map(|g| format!("{g:032x}")),
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| denied("grant store unavailable"))?;
        let expected_class = match root.dataset_class {
            DatasetClass::Synthetic => "synthetic",
            DatasetClass::Personal => "personal",
        };
        if class != expected_class {
            return Err(denied("dataset_class_mismatch"));
        }
        if active != 1 {
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
            loomward_catalog::sqlite_dir(&self.config.state_dir).join("state.db"),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| denied("grant store unavailable"))?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|_| denied("grant store unavailable"))?;
        type GrantRow = (String, String, Vec<u8>, String, String, Option<String>);
        let read_grant = |conn: &rusqlite::Connection| -> EngineResult<Option<GrantRow>> {
            conn.query_row(
                "SELECT g.display_path,g.volume_key,g.root_file_id,
                        (SELECT value FROM meta WHERE key='dataset_class'),g.identity_mode,g.volume_guid
                 FROM root_grant g WHERE g.id=?1 AND g.state='active'",
                [grant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()
            .map_err(|_| denied("grant store unavailable"))
        };
        let row = read_grant(&conn)?.ok_or_else(|| denied("grant_not_active"))?;
        let (path, volume_key, file_id, class, identity_mode, volume_guid) = row.clone();
        // #219: only the grant record allows identity-unverified listing, never the caller.
        let unverified = identity_mode == "unverified";
        // Wire IDs are opaque here; the authenticated identity must name this grant's root.
        let paired = rusqlite::Connection::open_with_flags(
            loomward_catalog::sqlite_dir(&self.config.state_dir).join("catalog.db"),
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
            let mut conn = conn;
            let mut row = row;
            let path = PathBuf::from(path);
            let source = loomward_windows::enumerate::NativeSource::default();
            let source = if unverified {
                source.with_identity_unverified()
            } else {
                source
            };
            #[cfg(test)]
            if let Some(hook) = tests::GRANT_OPEN_HOOK.with_borrow_mut(Option::take) {
                hook();
            }
            let (_, identity) = source
                .open_root(&path)
                .map_err(|_| denied("root open refused"))?;
            let same_id = match identity.id {
                Some(FileIdObs::Id128(id)) => file_id == id,
                Some(FileIdObs::Id64(id)) => file_id == id.to_le_bytes(),
                None => false,
            };
            let observed_guid = identity.volume_guid.map(|g| format!("{g:032x}"));
            if !same_id
                || identity.volume_serial.map(|s| format!("vsn_{s}")) != Some(volume_key)
                || observed_guid.is_none()
                || (volume_guid.is_some() && observed_guid != volume_guid)
            {
                return Err(denied("root_identity_changed"));
            }
            // Never hold a state writer lock during native I/O. Recheck the grant after opening;
            // legacy adoption additionally serialises its compare-and-set with revocation.
            row.5 = observed_guid.clone();
            if volume_guid.is_none() {
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|_| denied("grant store unavailable"))?;
                let changed = tx
                    .execute(
                        "UPDATE root_grant SET volume_guid=?1 WHERE id=?2 AND state='active'
                     AND volume_guid IS NULL AND volume_key=?3 AND root_file_id=?4
                     AND display_path=?5 AND identity_mode=?6",
                        rusqlite::params![observed_guid, grant_id, row.1, row.2, row.0, row.4],
                    )
                    .map_err(|_| denied("grant store unavailable"))?;
                if changed > 0 {
                    tx.execute("UPDATE revision SET state_rev=state_rev+1 WHERE id=1", [])
                        .map_err(|_| denied("grant store unavailable"))?;
                }
                let current = read_grant(&tx)?.ok_or_else(|| denied("grant_not_active"))?;
                if current != row {
                    return Err(denied("root_identity_changed"));
                }
                tx.commit().map_err(|_| denied("grant store unavailable"))?;
            } else {
                let current = read_grant(&conn)?.ok_or_else(|| denied("grant_not_active"))?;
                if current != row {
                    return Err(denied("root_identity_changed"));
                }
            }
            let mut root = GrantedRoot::new(root_id, path, dataset_class, identity);
            root.key = RootKey::Grant(grant_id);
            root.allow_identity_unverified = unverified;
            self.scan_root_gate(root.key())
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .aliases
                .insert(root.root_id().clone());
            Ok(root)
        }
        #[cfg(not(windows))]
        {
            let _ = (
                root_id,
                path,
                volume_key,
                volume_guid,
                file_id,
                dataset_class,
                unverified,
            );
            Err(EngineError::unavailable(Component::Scan))
        }
    }
    pub(crate) fn check_root(&self, root: &GrantedRoot) -> EngineResult<()> {
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
    /// Internal hint reconciliation; directory row IDs are resolved inside the granted root.
    pub fn scan_refresh_scopes(&self, root: &GrantedRoot, dirs: Vec<u64>) -> EngineResult<Job> {
        self.start_scan(root, None, RunScope::Targeted(dirs))
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
        // A durable revoke can precede its in-memory cancellation fence after root minting.
        let grant_active = || -> EngineResult<()> {
            if let RootKey::Grant(grant) = root.key() {
                let denied = |message: &str| EngineError::PermissionDenied {
                    message: message.into(),
                };
                let conn = rusqlite::Connection::open_with_flags(
                    loomward_catalog::sqlite_dir(&self.config.state_dir).join("state.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(|_| denied("grant store unavailable"))?;
                let active: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM root_grant WHERE id=?1 AND state='active')",
                        [grant],
                        |r| r.get(0),
                    )
                    .map_err(|_| denied("grant store unavailable"))?;
                if !active {
                    return Err(denied("grant_not_active"));
                }
            }
            Ok(())
        };
        grant_active()?;
        struct Starting(Arc<AtomicBool>);
        impl Drop for Starting {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        if revoked.starting.swap(true, Ordering::AcqRel) {
            return Err(EngineError::Busy {
                message: "root already starting".into(),
                detail: None,
            });
        }
        let _starting = Starting(revoked.starting.clone());
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
        // The reservation excludes other starts; cancellation can now acquire the gate.
        drop(revoked);
        // A sink that refuses watch hints (the trait default) is never watched: its scans are
        // plain full passes, and no watch pins outlive them (#184 MEDIUM-2).
        // An identity-unverified root (#219) is never watched: the watch reopens by file ID.
        #[cfg(windows)]
        let watched = root
            .expected
            .is_none_or(|e| e.basis != source::IdBasis::Unverified)
            && match sink.watch_dirty(root.root_id()) {
                Ok(()) => true,
                Err(EngineError::Unavailable { .. }) => false,
                Err(e) => return Err(e),
            };
        // A failed probe leaves the current watch installed and live. The start reservation
        // still excludes another start; revocation may independently remove and stop it.
        #[cfg(windows)]
        let existing = self.scan_watches.lock().unwrap().remove(root.root_id());
        #[cfg(windows)]
        if let Some(watch) = existing {
            watch.stop();
        }
        #[cfg(windows)]
        let armed = if watched {
            Some(watch::RootWatch::start(&root, sink.clone())?)
        } else {
            None
        };
        let revoked = gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if revoked.revoked {
            drop(revoked);
            #[cfg(windows)]
            if let Some(watch) = armed {
                watch.stop();
            }
            return Err(EngineError::PermissionDenied {
                message: "root revoked".into(),
            });
        }
        grant_active()?;
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
                let source = native_source(&root, io.clone());
                let report = pipeline::run_job_scan(
                    &source,
                    &root,
                    run,
                    sink.clone(),
                    bytes.clone(),
                    &ctx,
                    options.clone(),
                )?;
                if std::env::var_os("LOOMWARD_SCAN_PROFILE").is_some() {
                    eprintln!(
                        "scan-profile native {}",
                        loomward_windows::enumerate::scan_profile()
                    );
                }
                ctx.set_scan_report(&report)?;
                Ok(if report.coverage_complete() {
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
                let watch = armed.expect("watched roots arm before publication");
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
                let source = native_source(&root, io.clone());
                let report = watch::run_watched(
                    &source,
                    &root,
                    run,
                    sink.clone(),
                    bytes.clone(),
                    ctx.cancel.clone(),
                    options.clone(),
                    &watch,
                    Some(&ctx),
                )?;
                if std::env::var_os("LOOMWARD_SCAN_PROFILE").is_some() {
                    eprintln!(
                        "scan-profile native {}",
                        loomward_windows::enumerate::scan_profile()
                    );
                }
                ctx.set_scan_report(&report)?;
                Ok(if report.coverage_complete() {
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
    /// Acknowledge cancellation without waiting for directory I/O or writer joins. Once final
    /// publication is admitted, return the current job state; that outcome completes or fails.
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
        let cancelled = self.jobs.revoke_jobs(
            revoked
                .aliases
                .iter()
                .flat_map(|id| self.jobs.root_jobs_for_revocation(id)),
            &self.events,
        );
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
