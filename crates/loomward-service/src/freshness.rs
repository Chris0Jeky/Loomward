//! F3: persistent grant-scoped watches; journal hints select refresh scopes, never grant reach.
use crate::{db, now_ns, ts_ns, Inner};
use loomward_engine::scan::RunScope;
#[cfg(windows)]
use loomward_protocol::RootId;
use loomward_protocol::{SearchFreshness, SearchFreshnessState};
#[cfg(windows)]
use loomward_windows::usn::Bookmark;
#[cfg(windows)]
use rusqlite::{params, OptionalExtension};
use std::sync::Arc;
#[cfg(windows)]
mod reach;
#[cfg(windows)]
// sub_noid is the current root aggregate for files; the partial index covers directory gaps.
const GAP_SQL: &str = "SELECT sub_noid>0 OR EXISTS(
    SELECT 1 FROM main.dir INDEXED BY dir_missing_identity
    WHERE root_id=?1 AND file_id IS NULL AND listing_state!='absent_pending')
    FROM main.dir WHERE id=?2 AND root_id=?1 AND parent_id IS NULL";
#[cfg(windows)]
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, Weak,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct Tracker {
    #[cfg(windows)]
    enabled: AtomicBool,
    #[cfg(windows)]
    roots: Mutex<BTreeMap<i64, Tracked>>,
    #[cfg(windows)]
    stopped: AtomicBool,
    #[cfg(windows)]
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    #[cfg(windows)]
    setup: Mutex<()>,
    /// Roots an explicit scan or duplicates request is claiming. Held across every automatic start.
    #[cfg(windows)]
    explicit: Mutex<BTreeMap<i64, usize>>,
}

/// While held, no automatic refresh starts on `row`; `auto` names the one already started.
pub(crate) struct Explicit<'a> {
    #[cfg(windows)]
    tracker: &'a Tracker,
    #[cfg(not(windows))]
    tracker: std::marker::PhantomData<&'a Tracker>,
    #[cfg_attr(not(windows), allow(dead_code))]
    row: i64,
    pub(crate) auto: Option<loomward_protocol::JobId>,
}

impl Drop for Explicit<'_> {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let mut held = self.tracker.explicit.lock().unwrap();
            if let Some(n) = held.get_mut(&self.row) {
                *n -= 1;
                if *n == 0 {
                    held.remove(&self.row);
                }
            }
        }
    }
}

#[cfg(windows)]
struct Tracked {
    #[cfg(windows)]
    watch: Option<Arc<loomward_windows::watch::Watcher>>,
    checkpoint: Arc<Mutex<()>>,
    epoch: u64,
    stale_since: Option<i64>,
    at: i64,
    caught_up: bool,
    full: bool,
    paths: BTreeSet<Vec<Vec<u16>>>,
    targets: BTreeSet<u64>,
    auto: bool,
    attempts: u8,
    retry_after: Option<Instant>,
    changed: Instant,
    job: Option<loomward_protocol::JobId>,
    started: Option<(u64, Option<Bookmark>)>,
}

#[cfg(windows)]
impl Tracked {
    fn dirty(&mut self, full: bool) {
        self.epoch += 1;
        self.stale_since.get_or_insert(self.at);
        self.full |= full;
        self.auto = true;
        self.changed = Instant::now();
    }
}

impl Tracker {
    #[cfg(all(test, windows))]
    pub(crate) fn pause_pump(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(pump) = self.pump.lock().unwrap().take() {
            pump.join().unwrap();
        }
    }
    #[cfg(all(test, windows))]
    pub(crate) fn stop_watch(&self, row: i64) {
        let watch = self.roots.lock().unwrap()[&row].watch.clone().unwrap();
        watch.stop();
        assert!(watch.checkpoint().is_err());
    }
    #[cfg(all(test, windows))]
    pub(crate) fn ready_targeted_hint(&self, row: i64) {
        let mut roots = self.roots.lock().unwrap();
        let root = roots.get_mut(&row).unwrap();
        // The fixture starts idle; discard completed fallback bookkeeping, not newer events.
        root.job = None;
        root.full = false;
        root.changed = Instant::now() - Duration::from_secs(1);
    }
    #[cfg(all(test, windows))]
    pub(crate) fn unclaimed(&self, row: i64) -> bool {
        !self.explicit.lock().unwrap().contains_key(&row)
    }
    #[cfg(all(test, windows))]
    pub(crate) fn unclaimed_try(&self, row: i64) -> bool {
        self.explicit
            .try_lock()
            .is_ok_and(|held| !held.contains_key(&row))
    }
    #[cfg(all(test, windows))]
    pub(crate) fn checkpoint_fenced(&self, row: i64) -> bool {
        self.roots.lock().unwrap()[&row]
            .checkpoint
            .try_lock()
            .is_err()
    }
    #[cfg(all(test, windows))]
    pub(crate) fn exhaust_budget(&self, row: i64) {
        self.roots.lock().unwrap().get_mut(&row).unwrap().attempts = 3;
    }
    #[cfg(all(test, windows))]
    pub(crate) fn attempts(&self, row: i64) -> u8 {
        self.roots.lock().unwrap()[&row].attempts
    }
    #[cfg(all(test, windows))]
    pub(crate) fn expire_retry(&self, row: i64) {
        self.roots
            .lock()
            .unwrap()
            .get_mut(&row)
            .unwrap()
            .retry_after = Some(Instant::now());
    }
    #[cfg(all(test, windows))]
    pub(crate) fn debug(&self, row: i64) -> String {
        let roots = self.roots.lock().unwrap();
        roots.get(&row).map(|r|format!("epoch={} full={} auto={} attempts={} paths={} targets={} started={:?} failed={}",r.epoch,r.full,r.auto,r.attempts,r.paths.len(),r.targets.len(),r.started.map(|s|s.0),r.watch.as_ref().is_none_or(|w|w.failed()))).unwrap_or_default()
    }
    #[cfg(windows)]
    pub(crate) fn record(&self, row: i64, changes: Vec<loomward_windows::watch::Change>) {
        if let Some(root) = self.roots.lock().unwrap().get_mut(&row) {
            for change in changes {
                // Only a new filesystem hint after cooldown grants one retry.
                if root.attempts >= 3 && root.retry_after.is_some_and(|at| Instant::now() >= at) {
                    root.attempts = 2;
                    root.retry_after = None;
                }
                match change {
                    loomward_windows::watch::Change::RootDirty => root.dirty(true),
                    loomward_windows::watch::Change::Name { name, .. } => {
                        root.dirty(false);
                        let mut path: Vec<Vec<u16>> =
                            name.split(|c| *c == 92).map(<[u16]>::to_vec).collect();
                        path.pop();
                        if root.paths.len() < 1024 {
                            root.paths.insert(path);
                        } else {
                            root.full = true;
                        }
                    }
                }
            }
        }
    }
    pub(crate) fn stop(&self) {
        #[cfg(windows)]
        {
            self.stopped.store(true, Ordering::Release);
            if let Some(thread) = self.pump.lock().unwrap().take() {
                let _ = thread.join();
            }
            let _setup = self.setup.lock().unwrap();
            let roots = std::mem::take(&mut *self.roots.lock().unwrap());
            for root in roots.into_values() {
                if let Some(watch) = root.watch {
                    watch.stop();
                }
            }
        }
    }
    pub(crate) fn remove(&self, row: i64) {
        #[cfg(windows)]
        {
            // A revoke acknowledgement must also fence a watch whose native open is in flight.
            let _setup = self.setup.lock().unwrap();
            let old = self.roots.lock().unwrap().remove(&row);
            if let Some(root) = old {
                if let Some(watch) = root.watch {
                    watch.stop();
                }
            }
        }
        #[cfg(not(windows))]
        let _ = row;
    }
    /// Explicit owner work on `row` pre-empts automatic refreshes (#561, #568).
    pub(crate) fn explicit(&self, row: i64) -> Explicit<'_> {
        #[cfg(windows)]
        {
            let mut held = self.explicit.lock().unwrap();
            *held.entry(row).or_default() += 1;
            let auto = self
                .roots
                .lock()
                .unwrap()
                .get(&row)
                .and_then(|r| r.job.clone());
            Explicit {
                tracker: self,
                row,
                auto,
            }
        }
        #[cfg(not(windows))]
        Explicit {
            tracker: std::marker::PhantomData,
            row,
            auto: None,
        }
    }
    pub(crate) fn stale(&self, row: i64) -> bool {
        #[cfg(windows)]
        {
            self.roots
                .lock()
                .unwrap()
                .get(&row)
                .is_some_and(|r| r.stale_since.is_some())
        }
        #[cfg(not(windows))]
        {
            let _ = row;
            false
        }
    }
}

pub(crate) fn start(inner: &Arc<Inner>) {
    #[cfg(windows)]
    {
        inner.freshness.enabled.store(true, Ordering::Release);
        // Arm before returning the opened service, so its first scan cannot precede its watch.
        ensure_roots(inner);
        let weak = Arc::downgrade(inner);
        *inner.freshness.pump.lock().unwrap() = Some(std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(200));
            let Some(inner) = weak.upgrade() else { break };
            if inner.freshness.stopped.load(Ordering::Acquire) {
                break;
            }
            ensure_roots(&inner);
            refresh(&inner);
        }));
    }
    #[cfg(not(windows))]
    let _ = inner;
}

impl Inner {
    #[cfg(windows)]
    fn freshness_checkpoint(&self, row: i64) -> Option<bool> {
        let watched = self
            .freshness
            .roots
            .lock()
            .unwrap()
            .get(&row)
            .and_then(|r| r.watch.clone().map(|watch| (watch, r.checkpoint.clone())));
        let (watch, checkpoint) = watched?;
        #[cfg(test)]
        self.hooks.gate(
            loomward_protocol::Command::ScanStart,
            "freshness_checkpoint_waiting",
        );
        // The native checkpoint queue has one slot; search and publication share this watch.
        let _checkpoint = checkpoint.lock().unwrap();
        #[cfg(test)]
        self.hooks.gate(
            loomward_protocol::Command::ScanStart,
            "freshness_checkpoint_claimed",
        );
        Some(watch.checkpoint().is_ok() && !watch.failed())
    }
    pub(crate) fn hit_freshness(&self, row: i64) -> SearchFreshness {
        #[cfg(windows)]
        {
            if self.freshness_checkpoint(row) == Some(false) {
                if let Some(root) = self.freshness.roots.lock().unwrap().get_mut(&row) {
                    root.dirty(true);
                }
            }
        }
        #[cfg(windows)]
        if let Some(root) = self.freshness.roots.lock().unwrap().get(&row) {
            return match root.stale_since {
                Some(at) => SearchFreshness {
                    state: SearchFreshnessState::Stale,
                    at: Some(ts_ns(at)),
                },
                None if root.caught_up => SearchFreshness {
                    state: SearchFreshnessState::CaughtUp,
                    at: Some(ts_ns(root.at)),
                },
                None => SearchFreshness {
                    state: SearchFreshnessState::Live,
                    at: None,
                },
            };
        }
        // A fixture, offline root or unsupported volume has no watch coverage.
        let at = db::roots(&self.db(), Some(row))
            .ok()
            .and_then(|r| {
                r.first()
                    .map(|r| r.finished_at_ns.unwrap_or(r.granted_at_ns))
            })
            .unwrap_or_else(now_ns);
        SearchFreshness {
            state: SearchFreshnessState::Stale,
            at: Some(ts_ns(at)),
        }
    }

    pub(crate) fn freshness_begin(&self, grant: i64, scope: &RunScope) {
        #[cfg(windows)]
        {
            let row = db::root_for_grant(&self.db(), grant).ok().flatten();
            let Some(row) = row else { return };
            let bookmark = if matches!(scope, RunScope::FullRoot) {
                journal(self, row).and_then(|j| j.head().ok()).map(|h| h.0)
            } else {
                None
            };
            if let Some(root) = self.freshness.roots.lock().unwrap().get_mut(&row) {
                root.started = Some((root.epoch, bookmark));
                root.stale_since.get_or_insert(root.at);
                root.paths.clear();
                root.targets.clear();
            }
        }
        #[cfg(not(windows))]
        let _ = (grant, scope);
    }

    pub(crate) fn freshness_finish(&self, grant: i64, scope: &RunScope, complete: bool) {
        #[cfg(windows)]
        {
            let Some(row) = db::root_for_grant(&self.db(), grant).ok().flatten() else {
                return;
            };
            // Flush kernel notifications before comparing the scan's start epoch.
            let healthy = self.freshness_checkpoint(row).unwrap_or(false);
            let saved = {
                let mut roots = self.freshness.roots.lock().unwrap();
                let Some(root) = roots.get_mut(&row) else {
                    return;
                };
                let started = root.started.take();
                // A successful scan followed by newer hints is new work, not a failed retry.
                if complete && healthy && matches!(scope, RunScope::FullRoot) {
                    root.attempts = 0;
                    root.retry_after = None;
                }
                if complete
                    && matches!(scope, RunScope::FullRoot)
                    && healthy
                    && started.is_some_and(|s| s.0 == root.epoch)
                {
                    root.at = now_ns();
                    root.stale_since = None;
                    root.full = false;
                    root.auto = false;
                    root.attempts = 0;
                    root.caught_up = false;
                    Some((root.at, started.unwrap().1))
                } else {
                    None
                }
            };
            if let Some((at, bookmark)) = saved {
                // Save the pre-scan head only after publication. Replaying extra hints is safe;
                // saving a post-scan head could skip a change racing the last directory listing.
                if save(self, grant, bookmark, at).is_err() {
                    if let Some(root) = self.freshness.roots.lock().unwrap().get_mut(&row) {
                        root.dirty(true);
                        root.auto = false;
                    }
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (grant, scope, complete);
    }
}

#[cfg(windows)]
fn save(inner: &Inner, grant: i64, bookmark: Option<Bookmark>, at: i64) -> rusqlite::Result<()> {
    inner.db().execute("INSERT INTO st.root_freshness(grant_id,catalog_instance,journal_id,next_usn,at_ns) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(grant_id) DO UPDATE SET catalog_instance=excluded.catalog_instance,journal_id=excluded.journal_id,next_usn=excluded.next_usn,at_ns=excluded.at_ns",
        params![grant, inner.instance, bookmark.map(|b| b.journal_id.to_le_bytes().to_vec()), bookmark.map(|b| b.next_usn), at])?;
    Ok(())
}

#[cfg(windows)]
fn granted(inner: &Inner, row: &db::RootRow) -> Option<loomward_engine::scan::GrantedRoot> {
    inner.session_root_gate(row).ok()?;
    let id = RootId::new(inner.root_id(row.root)).ok()?;
    inner.scan.bind(&id, row.grant, row.dir?);
    inner
        .engine
        .granted_root(
            id,
            row.grant,
            &loomward_engine::scan::CatalogRootIdentity {
                row: row.root,
                instance: inner.instance.clone(),
            },
        )
        .ok()
}

#[cfg(windows)]
fn journal(inner: &Inner, row: i64) -> Option<loomward_windows::usn::Journal> {
    let row = db::roots(&inner.db(), Some(row)).ok()?.pop()?;
    let root = granted(inner, &row)?;
    loomward_windows::usn::Journal::open(root.path(), root.expected_identity()?).ok()
}

#[cfg(windows)]
pub(crate) fn ensure_roots(inner: &Arc<Inner>) {
    if !inner.freshness.enabled.load(Ordering::Acquire)
        || inner.freshness.stopped.load(Ordering::Acquire)
    {
        return;
    }
    let Ok(rows) = db::roots(&inner.db(), None) else {
        return;
    };
    for row in rows.iter().filter(|r| r.grant_state == "active") {
        let Ok(_setup) = inner.freshness.setup.try_lock() else {
            return;
        };
        if inner.freshness.stopped.load(Ordering::Acquire) {
            return;
        }
        if inner
            .freshness
            .roots
            .lock()
            .unwrap()
            .contains_key(&row.root)
        {
            continue;
        }
        let Some(grant) = granted(inner, row) else {
            continue;
        };
        let Some(expected) = grant.expected_identity() else {
            continue;
        };
        if expected.basis == loomward_windows::enumerate::IdBasis::Unverified {
            continue;
        }
        #[cfg(test)]
        inner.hooks.gate(
            loomward_protocol::Command::ScanStart,
            "persistent_watch_resolved",
        );
        // Every event after insertion must invalidate restart continuity, including watch setup.
        let epoch = 0;
        inner.freshness.roots.lock().unwrap().insert(
            row.root,
            Tracked {
                watch: None,
                checkpoint: Arc::default(),
                epoch,
                stale_since: Some(row.finished_at_ns.unwrap_or_else(now_ns)),
                at: row.finished_at_ns.unwrap_or_else(now_ns),
                caught_up: false,
                full: true,
                paths: BTreeSet::new(),
                targets: BTreeSet::new(),
                auto: false,
                attempts: 0,
                retry_after: None,
                changed: Instant::now(),
                job: None,
                started: None,
            },
        );
        let weak: Weak<Inner> = Arc::downgrade(inner);
        let root_row = row.root;
        let watch =
            loomward_windows::watch::Watcher::start(grant.path(), Some(expected), move |changes| {
                if let Some(inner) = weak.upgrade() {
                    inner.freshness.record(root_row, changes);
                }
            });
        if let Ok(watch) = watch {
            inner
                .freshness
                .roots
                .lock()
                .unwrap()
                .get_mut(&row.root)
                .unwrap()
                .watch = Some(Arc::new(watch));
            // Setup fences native handle creation only. Revoke must not wait for journal IO.
            drop(_setup);
            if row.finished_at_ns.is_some() {
                #[cfg(test)]
                inner.hooks.gate(
                    loomward_protocol::Command::ScanStart,
                    "persistent_watch_started",
                );
                catch_up(inner, row, epoch);
            }
        }
    }
}

#[cfg(windows)]
fn catch_up(inner: &Inner, row: &db::RootRow, epoch: u64) {
    if row.state != "complete" || !row.subtree_complete {
        return;
    }
    // shortcut: V2/FILE_ID_128 matching is qualified on NTFS only; qualify ReFS before using USN there.
    if !inner
        .root_filesystem(std::path::Path::new(&row.display_path))
        .is_ok_and(|fs| fs.eq_ignore_ascii_case("NTFS"))
    {
        return;
    }
    let saved: Option<(Option<Vec<u8>>, Option<i64>, i64)> = inner.db().query_row(
        "SELECT journal_id,next_usn,at_ns FROM st.root_freshness WHERE grant_id=?1 AND catalog_instance=?2", params![row.grant, inner.instance],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().ok().flatten();
    let Some((Some(id), Some(next_usn), at)) = saved else {
        // A pre-F3 scan has no continuity witness. Only a full refresh can repair it.
        return;
    };
    let Ok(id) = <[u8; 8]>::try_from(id) else {
        return;
    };
    let Some(journal) = journal(inner, row.root) else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    #[cfg(test)]
    inner.hooks.gate(
        loomward_protocol::Command::ScanStart,
        "persistent_journal_read",
    );
    // The native reader is bounded to 16 MiB. Decode before taking any catalogue lock.
    let mut records = Vec::new();
    let result = journal.catch_up(
        Bookmark {
            journal_id: u64::from_le_bytes(id),
            next_usn,
        },
        |record| records.push(record),
    );
    replay(inner, row, epoch, at, deadline, result, &records);
}

#[cfg(windows)]
fn replay(
    inner: &Inner,
    row: &db::RootRow,
    epoch: u64,
    at: i64,
    deadline: Instant,
    result: Result<Bookmark, loomward_windows::enumerate::SourceError>,
    records: &[loomward_windows::usn::Record],
) {
    let targets = result.as_ref().ok().and_then(|_| {
        let missing_identity = inner
            .db()
            .query_row(GAP_SQL, params![row.root, row.dir?], |r| r.get(0))
            .ok()?;
        reach::targets(records, deadline, missing_identity, |ids| {
            if !inner
                .freshness
                .roots
                .lock()
                .unwrap()
                .contains_key(&row.root)
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            reach::lookup(&inner.db(), row.root, ids)
        })
    });
    let mut roots = inner.freshness.roots.lock().unwrap();
    let Some(root) = roots.get_mut(&row.root) else {
        return;
    };
    root.stale_since = Some(at);
    if let (Ok(head), Some(targets)) = (result, targets) {
        if root.epoch == epoch {
            root.full = false;
        }
        root.targets = targets;
        if root.targets.is_empty() && root.epoch == epoch {
            root.stale_since = None;
            root.at = now_ns();
            root.caught_up = true;
            let at = root.at;
            drop(roots);
            if save(inner, row.grant, Some(head), at).is_err() {
                if let Some(root) = inner.freshness.roots.lock().unwrap().get_mut(&row.root) {
                    root.dirty(true);
                    root.auto = false;
                }
            }
        } else {
            root.auto = true;
        }
        return;
    }
    // Wrapped/recreated/unsupported journals and read budgets are loss of coverage, never empty.
    root.full = true;
    root.auto = false;
}

#[cfg(windows)]
pub(crate) fn refresh(inner: &Inner) {
    let rows = db::roots(&inner.db(), None).unwrap_or_default();
    for row in rows {
        if row.grant_state != "active" {
            inner.freshness.remove(row.root);
            continue;
        }
        let action = {
            let mut roots = inner.freshness.roots.lock().unwrap();
            let Some(root) = roots.get_mut(&row.root) else {
                continue;
            };
            if let Some(job) = &root.job {
                if let Ok(job) = inner.engine.job_status(job) {
                    if matches!(
                        job.state,
                        loomward_protocol::JobState::Queued
                            | loomward_protocol::JobState::Running
                            | loomward_protocol::JobState::CancelRequested
                    ) {
                        continue;
                    }
                    // Owner pre-emption is deferred work, not a failed automatic attempt.
                    if job.state == loomward_protocol::JobState::Cancelled {
                        root.attempts = root.attempts.saturating_sub(1);
                    }
                }
                root.job = None;
                // Targeted scans deliberately leave root coverage partial. One full fallback
                // establishes a clean baseline before claiming live/caught_up.
                root.full = true;
            }
            if !root.auto
                || root.attempts >= 3
                || root.stale_since.is_none()
                || root.changed.elapsed() < Duration::from_millis(500)
            {
                continue;
            }
            (root.full, root.paths.clone(), root.targets.clone())
        };
        let Some(grant) = granted(inner, &row) else {
            inner.freshness.remove(row.root);
            continue;
        };
        let mut targets = action.2;
        let mut full = action.0;
        for path in action.1 {
            match resolve(inner, row.root, &path) {
                Some(dir) => {
                    targets.insert(dir);
                }
                None => full = true,
            }
        }
        // Start and record under the claim fence, so an explicit request sees every auto job.
        let claimed = inner.freshness.explicit.lock().unwrap();
        if claimed.contains_key(&row.root) {
            continue;
        }
        #[cfg(test)]
        inner.hooks.gate(
            loomward_protocol::Command::ScanStart,
            "automatic_claim_checked",
        );
        // Count before submission: a fast worker may finish and reset the budget before return.
        if let Some(root) = inner.freshness.roots.lock().unwrap().get_mut(&row.root) {
            root.attempts += 1;
            if root.attempts >= 3 {
                root.retry_after = Some(Instant::now() + Duration::from_secs(30));
            }
        }
        #[cfg(test)]
        let failed = inner
            .hooks
            .fail_automatic_start
            .swap(false, Ordering::SeqCst);
        #[cfg(not(test))]
        let failed = false;
        let job = if failed {
            Err(loomward_engine::EngineError::Internal {
                message: "synthetic automatic failure".into(),
                detail: None,
            })
        } else if full || targets.is_empty() {
            inner.engine.scan_refresh(&grant, None)
        } else {
            inner
                .engine
                .scan_refresh_scopes(&grant, targets.into_iter().collect())
        };
        #[cfg(test)]
        inner
            .hooks
            .gate(loomward_protocol::Command::ScanStart, "automatic_submitted");
        if let Some(root) = inner.freshness.roots.lock().unwrap().get_mut(&row.root) {
            match job {
                Ok(job) => {
                    root.job = Some(job.job_id);
                }
                Err(loomward_engine::EngineError::Busy { .. }) => {
                    root.attempts = root.attempts.saturating_sub(1);
                }
                Err(_) => {
                    root.changed = Instant::now();
                }
            }
        }
    }
}

#[cfg(windows)]
const RESOLVE_CHILD_SQL: &str = "SELECT id FROM main.dir WHERE parent_id=?1 AND name=?2 AND listing_state NOT IN ('absent_pending','excluded')";

#[cfg(windows)]
fn resolve(inner: &Inner, root: i64, path: &[Vec<u16>]) -> Option<u64> {
    let conn = inner.db();
    let mut dir: u64 = conn
        .query_row(
            "SELECT id FROM main.dir WHERE root_id=?1 AND parent_id IS NULL",
            [root],
            |r| r.get::<_, i64>(0).map(|v| v as u64),
        )
        .ok()?;
    let mut lookup = conn.prepare_cached(RESOLVE_CHILD_SQL).ok()?;
    for name in path {
        let text = String::from_utf16(name).ok()?;
        dir = lookup
            .query_row(params![dir as i64, text], |r| {
                r.get::<_, i64>(0).map(|v| v as u64)
            })
            .ok()?;
    }
    Some(dir)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn refresh_path_components_reuse_the_prepared_lookup_and_stay_contained() {
        let temp = crate::tests::temp_root();
        let svc = crate::Service::open(crate::Config {
            state_dir: temp.path().to_path_buf(),
            dataset: loomward_protocol::DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        svc.inner.freshness.stop();
        let (root, _) = crate::tests::fixture(&svc, 0);
        let row = db::roots(&svc.inner.db(), Some(root)).unwrap().remove(0);
        let mut leaf = row.dir.unwrap() as i64;
        let mut path = Vec::new();
        for depth in 1..=20 {
            let name = format!("synthetic-\u{3bb}-{depth}");
            let conn = svc.inner.db();
            conn.execute("INSERT INTO main.dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run) VALUES(?1,?2,?3,'path_observation','none',?4,0,'complete',0,0)", params![root,leaf,name,depth]).unwrap();
            leaf = conn.last_insert_rowid();
            path.push(name.encode_utf16().collect());
        }
        for _ in 0..3 {
            assert_eq!(resolve(&svc.inner, root, &path), Some(leaf as u64));
        }
        assert_eq!(
            svc.inner
                .db()
                .prepare_cached(RESOLVE_CHILD_SQL)
                .unwrap()
                .get_status(rusqlite::StatementStatus::Run),
            60,
            "all path components must reuse the same prepared statement"
        );
        for state in ["absent_pending", "excluded"] {
            svc.inner
                .db()
                .execute(
                    "UPDATE main.dir SET listing_state=?1 WHERE id=?2",
                    params![state, leaf - 1],
                )
                .unwrap();
            assert_eq!(resolve(&svc.inner, root, &path), None);
        }
        assert_eq!(resolve(&svc.inner, root, &[vec![0xd800]]), None);
        assert_eq!(resolve(&svc.inner, i64::MAX, &[]), None);
    }

    #[test]
    fn f3_complete_verified_root_with_null_directory_stays_stale_on_unresolved_record() {
        let temp = crate::tests::temp_root();
        let svc = crate::Service::open(crate::Config {
            state_dir: temp.path().to_path_buf(),
            dataset: loomward_protocol::DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        svc.inner.freshness.stop();
        let (root_id, _) = crate::tests::fixture(&svc, 0);
        let row = db::roots(&svc.inner.db(), Some(root_id)).unwrap().remove(0);
        assert_eq!(row.state, "complete");
        assert!(row.subtree_complete);
        let observed: (String, i64, i64) = svc.inner.db().query_row(
            "SELECT g.identity_mode,d.sub_noid,(SELECT count(*) FROM main.dir c WHERE c.root_id=d.root_id AND c.file_id IS NULL) FROM main.dir d JOIN main.root r ON r.id=d.root_id JOIN st.root_grant g ON g.id=r.grant_id WHERE d.parent_id IS NULL",
            [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(observed, ("verified".into(), 0, 1));
        let records = [loomward_windows::usn::Record {
            file_id: 99u64.to_le_bytes().to_vec(),
            parent_id: 999u64.to_le_bytes().to_vec(),
            reason: 0x100,
            is_directory: false,
        }];
        // Also retain the identified-root control and the conservative sub_noid guard.
        for missing in ["directory", "none", "sub_noid"] {
            if missing == "none" {
                svc.inner
                    .db()
                    .execute(
                        "UPDATE main.dir SET file_id=zeroblob(16) WHERE file_id IS NULL",
                        [],
                    )
                    .unwrap();
            } else if missing == "sub_noid" {
                svc.inner
                    .db()
                    .execute("UPDATE main.dir SET sub_noid=1 WHERE parent_id IS NULL", [])
                    .unwrap();
            }
            svc.inner.freshness.roots.lock().unwrap().insert(
                root_id,
                Tracked {
                    watch: None,
                    checkpoint: Arc::default(),
                    epoch: 0,
                    stale_since: Some(1),
                    at: 1,
                    caught_up: false,
                    full: true,
                    paths: BTreeSet::new(),
                    targets: BTreeSet::new(),
                    auto: false,
                    attempts: 3,
                    retry_after: None,
                    changed: Instant::now(),
                    job: None,
                    started: None,
                },
            );
            replay(
                &svc.inner,
                &row,
                0,
                1,
                Instant::now() + Duration::from_secs(5),
                Ok(Bookmark {
                    journal_id: 1,
                    next_usn: 2,
                }),
                &records,
            );
            let (caught_up, stale, full) = {
                let roots = svc.inner.freshness.roots.lock().unwrap();
                let tracked = &roots[&root_id];
                (
                    tracked.caught_up,
                    tracked.stale_since.is_some(),
                    tracked.full,
                )
            };
            if missing == "none" {
                assert!(caught_up && !stale);
            } else {
                assert!(stale, "{missing}: unresolved change falsely caught up");
                assert!(
                    full && !caught_up,
                    "{missing}: missing full-root dirty scope"
                );
            }
        }
        assert_eq!(
            svc.inner.hit_freshness(root_id).state,
            SearchFreshnessState::Stale
        );
    }

    // Retain the reviewed query only as the before-measurement, never on the production path.
    const OLD_PARENT: &str = "WITH RECURSIVE visible(id) AS (SELECT id FROM main.dir WHERE root_id=?1 AND parent_id IS NULL UNION ALL SELECT d.id FROM main.dir d JOIN visible v ON d.parent_id=v.id WHERE d.listing_state NOT IN ('absent_pending','excluded')) SELECT d.id FROM main.dir d JOIN visible v ON v.id=d.id WHERE d.file_id=?2 OR (length(d.file_id)=8 AND d.file_id=substr(?2,1,8) AND substr(?2,9,8)=zeroblob(8)) LIMIT 1";

    pub(super) fn catalogue(dirs: u64) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../loomward-catalog/src/catalog.sql"))
            .unwrap();
        conn.execute_batch("BEGIN; INSERT INTO root(id,grant_id,state) VALUES(1,1,'complete');")
            .unwrap();
        {
            let mut insert = conn.prepare("INSERT INTO dir(id,root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run) VALUES(?1,1,?2,?3,?4,'native_file_id_128','post_open',?5,0,'complete',1,1)").unwrap();
            for id in 1..=dirs {
                let mut native = id.to_le_bytes().to_vec();
                native.resize(16, 0);
                insert
                    .execute(params![
                        id as i64,
                        (id != 1).then_some(1),
                        format!("synthetic-{id}"),
                        native,
                        i64::from(id != 1)
                    ])
                    .unwrap();
            }
        }
        conn.execute_batch("COMMIT").unwrap();
        conn
    }

    #[test]
    fn f3_identity_resolution_uses_directory_identity_index() {
        let conn = catalogue(10);
        let mut id = 10u64.to_le_bytes().to_vec();
        id.resize(16, 0);
        let (sql, args) = reach::lookup_sql(&BTreeSet::from([id]), 1);
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let plan: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(args), |r| r.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|line| line.contains("dir_identity") && line.contains("file_id=?")),
            "{plan:#?}"
        );
        assert!(
            plan.iter()
                .any(|line| line.contains("file_identity") && line.contains("file_id=?")),
            "{plan:#?}"
        );
        eprintln!("F3 indexed query plan: {plan:#?}");
    }

    #[test]
    fn f3_gap_probe_ignores_retired_identity_but_preserves_live_gaps() {
        let conn = catalogue(4);
        let gap = || {
            conn.query_row(GAP_SQL, params![1, 1], |r| r.get::<_, bool>(0))
                .unwrap()
        };
        assert!(!gap());
        conn.execute_batch("UPDATE dir SET listing_state='excluded' WHERE id=2;
            UPDATE dir SET parent_id=2,listing_state='absent_pending',file_id=NULL,sub_noid=7 WHERE id=3").unwrap();
        assert!(
            !gap(),
            "retired rows below an excluded directory are not identity gaps"
        );
        conn.execute_batch("UPDATE dir SET file_id=NULL WHERE id=4")
            .unwrap();
        assert!(gap(), "a visible NULL directory still needs a full refresh");
        conn.execute_batch(
            "UPDATE dir SET file_id=zeroblob(16) WHERE id=4;
            UPDATE dir SET sub_noid=1 WHERE id=1",
        )
        .unwrap();
        assert!(gap(), "the root aggregate must retain file identity gaps");
    }

    #[test]
    fn f3_gap_probe_scale() {
        let dirs = if std::env::var_os("LOOMWARD_F3_GAP_SCALE").is_some() {
            1_000_000
        } else {
            10
        };
        let conn = catalogue(dirs);
        let records = [loomward_windows::usn::Record {
            file_id: (dirs + 1).to_le_bytes().to_vec(),
            parent_id: dirs.to_le_bytes().to_vec(),
            reason: 0x100,
            is_directory: false,
        }];
        let old = "SELECT EXISTS(SELECT 1 FROM main.dir WHERE root_id=?1 AND (file_id IS NULL OR sub_noid>0))";
        for (label, sql) in [("before", old), ("after", GAP_SQL)] {
            let mut stmt = conn.prepare(sql).unwrap();
            let args = [1i64, 1];
            let args = &args[..stmt.parameter_count()];
            let began = Instant::now();
            let missing: bool = stmt
                .query_row(rusqlite::params_from_iter(args), |r| r.get(0))
                .unwrap();
            let elapsed = began.elapsed();
            let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
            eprintln!("F3 gap {label}: {dirs} directories, {elapsed:?}, {steps} VM steps");
            assert!(!missing);
            if label == "after" {
                assert!(
                    steps < 100,
                    "gap probe must not scale with directory count: {steps}"
                );
            }
            for counted in [false, true] {
                let steps = Arc::new(std::sync::atomic::AtomicU64::new(0));
                if counted {
                    let count = steps.clone();
                    conn.progress_handler(
                        1,
                        Some(move || {
                            count.fetch_add(1, Ordering::Relaxed);
                            false
                        }),
                    )
                    .unwrap();
                }
                let began = Instant::now();
                let missing = stmt
                    .query_row(rusqlite::params_from_iter(args), |r| r.get(0))
                    .unwrap();
                let targets =
                    reach::targets(&records, began + Duration::from_secs(30), missing, |ids| {
                        reach::lookup(&conn, 1, ids)
                    });
                let elapsed = began.elapsed();
                conn.progress_handler(0, None::<fn() -> bool>).unwrap();
                eprintln!("F3 catch-up {label}: {dirs} directories, step-counted={counted}, {elapsed:?}, {} VM steps", steps.load(Ordering::Relaxed));
                assert_eq!(targets, Some(BTreeSet::from([dirs])));
            }
        }
    }

    #[test]
    fn f3_catch_up_100k_directories() {
        let conn = catalogue(100_000);
        let records: Vec<_> = (99_873u64..=100_000)
            .map(|id| loomward_windows::usn::Record {
                file_id: (id + 100_000).to_le_bytes().to_vec(),
                parent_id: id.to_le_bytes().to_vec(),
                reason: 0x100,
                is_directory: false,
            })
            .collect();
        let expected: BTreeSet<u64> = (99_873..=100_000).collect();
        // The slow before-measurement is opt-in; normal suites still prove the 100k indexed path.
        if std::env::var_os("LOOMWARD_F3_BASELINE").is_some() {
            let began = Instant::now();
            let mut before = BTreeSet::new();
            for record in &records {
                let mut native = record.parent_id.clone();
                native.resize(16, 0);
                let parent: i64 = conn
                    .query_row(OLD_PARENT, params![1, native], |r| r.get(0))
                    .unwrap();
                before.insert(parent as u64);
                let mut file = record.file_id.clone();
                file.resize(16, 0);
                let previous: Option<i64> = conn.query_row("SELECT parent_id FROM main.dir WHERE root_id=?1 AND (file_id=?2 OR (length(file_id)=8 AND file_id=substr(?2,1,8) AND substr(?2,9,8)=zeroblob(8))) AND parent_id IS NOT NULL UNION ALL SELECT f.dir_id FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE d.root_id=?1 AND (f.file_id=?2 OR (length(f.file_id)=8 AND f.file_id=substr(?2,1,8) AND substr(?2,9,8)=zeroblob(8))) LIMIT 1", params![1,file], |r| r.get(0)).optional().unwrap();
                if let Some(previous) = previous {
                    before.insert(previous as u64);
                }
            }
            eprintln!(
                "F3 before: 100000 directories, 128 decoded records: {:?}",
                began.elapsed()
            );
            assert_eq!(before, expected);
        }
        let began = Instant::now();
        let after = reach::targets(&records, began + Duration::from_secs(5), false, |ids| {
            reach::lookup(&conn, 1, ids)
        });
        eprintln!(
            "F3 after: 100000 directories, 128 decoded records: {:?}",
            began.elapsed()
        );
        assert_eq!(after, Some(expected));
    }
}
