//! Narrow staged writer seam. L2 supplies durability; the fixture sink keeps only aggregates.
use super::source::{FileIdObs, IdBasis, ListOutcome, OpenedIdentity};
use crate::{EngineError, EngineResult};
use loomward_protocol::RootId;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

/// Coverage reconciliation scope. Targeted runs cannot sweep outside their named directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunScope {
    /// Complete traversal of the granted root.
    FullRoot,
    /// Explicit directory identities only; unresolved tombstones elsewhere remain.
    Targeted(Vec<u64>),
}
/// A sink-owned directory and the revision reserved for this particular listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingTicket {
    /// Catalogue directory identifier, never a scan-local path hash.
    pub dir: u64,
    /// Revision this listing and its final were computed from (#117 N1).
    pub input_revision: u64,
    /// Previous parent if identity reconciliation moved this directory.
    pub old_parent: Option<u64>,
}
/// Owned compact metadata entry. No content, path reconstruction or operation capability.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Exact name, preserving unpaired UTF-16 surrogates.
    pub name: Vec<u16>,
    /// Width-preserving native observation.
    pub file_id: Option<FileIdObs>,
    /// Whether identity may be used for unique matching (false for ReFS 64-bit IDs).
    pub identity_eligible: bool,
    /// Listed attributes.
    pub attributes: u32,
    /// Reparse tag, if reported.
    pub reparse_tag: Option<u32>,
    /// Logical default stream bytes.
    pub logical: u64,
    /// Default stream allocation, unknown preserved.
    pub allocated: Option<u64>,
    /// Creation ticks.
    pub creation: Option<i64>,
    /// Modification ticks.
    pub last_write: Option<i64>,
    /// Change ticks.
    pub change: Option<i64>,
    /// Access ticks.
    pub last_access: Option<i64>,
    /// Entry was deliberately excluded from traversal.
    pub excluded: bool,
    /// A child could not be validated/opened; the catalogue preserves its denied/partial state.
    pub traversal_error: Option<super::source::SourceError>,
}
/// Checked entry-count and byte totals, allocation unknownness retained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sums {
    /// File entries.
    pub files: u64,
    /// Descendant directories.
    pub dirs: u64,
    /// Default-stream logical bytes.
    pub logical: u64,
    /// Observed allocated bytes, or unknown if any file lacked it.
    pub allocated: Option<u64>,
}
impl Sums {
    /// Known-empty totals.
    pub const ZERO: Self = Self {
        files: 0,
        dirs: 0,
        logical: 0,
        allocated: Some(0),
    };
    /// Add without wrapping bytes or counts.
    pub fn checked_add(self, b: Self) -> EngineResult<Self> {
        let overflow = || EngineError::Internal {
            message: "byte_overflow".into(),
            detail: Some(
                loomward_protocol::Detail::new(std::collections::BTreeMap::from([(
                    "reason".into(),
                    serde_json::json!("byte_overflow"),
                )]))
                .unwrap(),
            ),
        };
        Ok(Self {
            files: self.files.checked_add(b.files).ok_or_else(overflow)?,
            dirs: self.dirs.checked_add(b.dirs).ok_or_else(overflow)?,
            logical: self.logical.checked_add(b.logical).ok_or_else(overflow)?,
            allocated: match (self.allocated, b.allocated) {
                (Some(a), Some(b)) => Some(a.checked_add(b).ok_or_else(overflow)?),
                _ => None,
            },
        })
    }
}
/// One writer message; all directory publication is deferred until ListingDone.
#[derive(Debug)]
pub enum ScanMessage {
    /// Append one bounded chunk to staging, tagged by its listing's input revision.
    DirListing {
        /// Active run tag.
        run: u64,
        /// Exact listing revision.
        ticket: ListingTicket,
        /// Compact metadata chunk.
        entries: Vec<Entry>,
    },
    /// Atomically publish staged entries; incomplete outcomes only permit upserts.
    ListingDone {
        /// Active run tag.
        run: u64,
        /// Exact listing revision.
        ticket: ListingTicket,
        /// Complete only on native EOF.
        outcome: ListOutcome,
    },
    /// Accept only for the active run and exact dirty input revision.
    DirFinal {
        /// Active run tag.
        run: u64,
        /// Exact listing revision.
        ticket: ListingTicket,
        /// Checked subtree totals.
        sums: Sums,
        /// Subtree coverage is complete.
        complete: bool,
    },
    /// Reparent invalidates both chains even if the run cancels (#117 partial 8).
    InvalidateChains {
        /// Active run tag.
        run: u64,
        /// Old parent chain.
        old_parent: u64,
        /// New parent chain.
        new_parent: u64,
    },
}
/// Catalogue writer contract. Implementations must fence before accepting any later message.
pub trait ScanSink: Send + Sync {
    /// Recover durable running jobs to failed/repairing before any root serves slices.
    fn recover_interrupted(&self) -> EngineResult<()>;
    /// Begin one active root run; record its reconciliation scope.
    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> EngineResult<()>;
    /// Resolve a directory and reserve a listing revision. Executed on the writer thread.
    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> EngineResult<ListingTicket>;
    /// Reserve a fresh dirty epoch for a rejected listing without changing its identity or parent.
    /// Called on the scan coordinator thread while the writer is idle after a barrier, not on
    /// the writer thread: an implementation must not rely on writer-thread affinity.
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> EngineResult<ListingTicket>;
    /// Stage/publish/finalize a message, rejecting obsolete runs and revisions.
    fn consume(&self, root: &RootId, message: ScanMessage) -> EngineResult<()>;
    /// Finish: only full-root plus complete permits global absence reconciliation.
    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
    ) -> EngineResult<()>;
    /// After durable revocation commits, reject all subsequently arriving writer messages.
    fn fence_root(&self, root: &RootId) -> EngineResult<()>;
}
#[derive(Debug, Default)]
struct MemoryState {
    next_dir: u64,
    next_rev: u64,
    roots: HashMap<RootId, MemoryRoot>,
}
#[derive(Debug, Default)]
struct MemoryRoot {
    active: Option<u64>,
    fenced: bool,
    repairing: bool,
    sweep: bool,
    dirs: HashMap<u64, MemoryDir>,
    stages: HashMap<(u64, u64, u64), Sums>,
    invalidated: HashSet<u64>,
}
#[derive(Debug)]
struct MemoryDir {
    parent: Option<u64>,
    name: Vec<u16>,
    id: Option<FileIdObs>,
    dirty: u64,
    dirty_run: u64,
    valid: u64,
    own: Sums,
    sums: Sums,
}
/// In-memory fixture writer; retains aggregates and directories, never a million-entry snapshot.
#[derive(Debug, Default)]
pub struct MemorySink {
    state: Mutex<MemoryState>,
}
impl MemorySink {
    /// Last accepted subtree totals for the root, once its final was current.
    pub fn totals(&self, root: &RootId) -> Option<Sums> {
        self.state
            .lock()
            .unwrap()
            .roots
            .get(root)?
            .dirs
            .values()
            .find(|d| d.parent.is_none())
            .filter(|d| d.valid == d.dirty)
            .map(|d| d.sums)
    }
    /// Whether root-wide sweeping was authorized by the last finish.
    pub fn sweep_allowed(&self, root: &RootId) -> bool {
        self.state
            .lock()
            .unwrap()
            .roots
            .get(root)
            .is_some_and(|r| r.sweep)
    }
    /// A restart left an interrupted root in repairing, never immediately consistent.
    pub fn is_repairing(&self, root: &RootId) -> bool {
        self.state
            .lock()
            .unwrap()
            .roots
            .get(root)
            .is_some_and(|r| r.repairing)
    }
    /// Both chains recorded by reparent reconciliation.
    pub fn invalidated(&self, root: &RootId) -> Vec<u64> {
        self.state
            .lock()
            .unwrap()
            .roots
            .get(root)
            .map(|r| r.invalidated.iter().copied().collect())
            .unwrap_or_default()
    }
    /// Directory revision currently invalidated, for relist/watcher fixture tests.
    pub fn dirty_again(&self, root: &RootId, dir: u64) {
        let mut s = self.state.lock().unwrap();
        s.next_rev += 1;
        let revision = s.next_rev;
        s.roots
            .get_mut(root)
            .unwrap()
            .dirs
            .get_mut(&dir)
            .unwrap()
            .dirty = revision;
    }
}
fn stale() -> EngineError {
    EngineError::StaleGeneration {
        message: "obsolete scan writer message".into(),
        detail: Some(
            loomward_protocol::Detail::new(std::collections::BTreeMap::from([(
                "reason".into(),
                serde_json::json!("listing_revision_changed"),
            )]))
            .unwrap(),
        ),
    }
}
fn active(r: &MemoryRoot, run: u64) -> EngineResult<()> {
    if r.fenced {
        return Err(EngineError::PermissionDenied {
            message: "root revoked".into(),
        });
    }
    if r.active != Some(run) {
        return Err(EngineError::StaleGeneration {
            message: "obsolete scan run".into(),
            detail: None,
        });
    }
    Ok(())
}
fn invalidate(r: &mut MemoryRoot, mut dir: u64, revision: Option<u64>) {
    for _ in 0..129 {
        r.invalidated.insert(dir);
        let Some(d) = r.dirs.get_mut(&dir) else { break };
        d.valid = 0;
        if let Some(revision) = revision {
            d.dirty = revision;
        }
        let Some(parent) = d.parent else { break };
        dir = parent;
    }
}
impl ScanSink for MemorySink {
    fn recover_interrupted(&self) -> EngineResult<()> {
        let mut s = self.state.lock().unwrap();
        for r in s.roots.values_mut() {
            if r.active.take().is_some() {
                r.repairing = true;
                r.sweep = false;
                for d in r.dirs.values_mut() {
                    d.valid = 0;
                }
                r.stages.clear();
            }
        }
        Ok(())
    }
    fn begin_run(&self, root: &RootId, run: u64, _scope: &RunScope) -> EngineResult<()> {
        let mut s = self.state.lock().unwrap();
        let r = s.roots.entry(root.clone()).or_default();
        if r.fenced {
            return Err(EngineError::PermissionDenied {
                message: "root revoked".into(),
            });
        }
        if r.active.is_some() {
            return Err(EngineError::Busy {
                message: "root writer busy".into(),
                detail: None,
            });
        }
        r.active = Some(run);
        r.sweep = false;
        Ok(())
    }
    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> EngineResult<ListingTicket> {
        let mut s = self.state.lock().unwrap();
        active(s.roots.get(root).ok_or_else(stale)?, run)?;
        s.next_rev += 1;
        let revision = s.next_rev;
        let eligible = matches!(identity.basis, IdBasis::Listed | IdBasis::PostOpen);
        let id = identity.id.filter(|_| eligible);
        let found = s.roots[root]
            .dirs
            .iter()
            .find(|(_, d)| {
                if let Some(id) = id {
                    d.id == Some(id)
                } else {
                    d.parent == parent && d.name == name && d.id.is_none()
                }
            })
            .map(|(id, _)| *id);
        let dir = found.unwrap_or_else(|| {
            s.next_dir += 1;
            s.next_dir
        });
        let r = s.roots.get_mut(root).unwrap();
        let old_parent = r
            .dirs
            .get(&dir)
            .and_then(|d| d.parent)
            .filter(|p| Some(*p) != parent);
        let d = r.dirs.entry(dir).or_insert(MemoryDir {
            parent,
            name: name.to_vec(),
            id,
            dirty: revision,
            dirty_run: run,
            valid: 0,
            own: Sums::ZERO,
            sums: Sums::ZERO,
        });
        d.parent = parent;
        d.dirty = revision;
        d.dirty_run = run;
        d.name = name.to_vec();
        d.id = id;
        Ok(ListingTicket {
            dir,
            input_revision: revision,
            old_parent,
        })
    }
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> EngineResult<ListingTicket> {
        let mut s = self.state.lock().unwrap();
        active(s.roots.get(root).ok_or_else(stale)?, run)?;
        s.next_rev += 1;
        let revision = s.next_rev;
        let r = s.roots.get_mut(root).unwrap();
        let d = r.dirs.get_mut(&ticket.dir).ok_or_else(stale)?;
        if d.dirty_run != run {
            return Err(EngineError::StaleGeneration {
                message: "obsolete scan run".into(),
                detail: None,
            });
        }
        d.dirty = revision;
        d.valid = 0;
        r.stages.retain(|(_, dir, _), _| *dir != ticket.dir);
        Ok(ListingTicket {
            input_revision: revision,
            old_parent: None,
            ..ticket
        })
    }
    fn consume(&self, root: &RootId, message: ScanMessage) -> EngineResult<()> {
        let mut s = self.state.lock().unwrap();
        let MemoryState {
            next_rev, roots, ..
        } = &mut *s;
        let r = roots.get_mut(root).ok_or_else(stale)?;
        match message {
            ScanMessage::DirListing {
                run,
                ticket,
                entries,
            } => {
                active(r, run)?;
                if r.dirs[&ticket.dir].dirty != ticket.input_revision {
                    return Err(stale());
                }
                let staged = r
                    .stages
                    .entry((run, ticket.dir, ticket.input_revision))
                    .or_insert(Sums::ZERO);
                for e in entries {
                    if !e.excluded && e.attributes & 0x10 == 0 {
                        *staged = staged.checked_add(Sums {
                            files: 1,
                            dirs: 0,
                            logical: e.logical,
                            allocated: e.allocated,
                        })?;
                    }
                }
            }
            ScanMessage::ListingDone {
                run,
                ticket,
                outcome: _,
            } => {
                active(r, run)?;
                if r.dirs[&ticket.dir].dirty != ticket.input_revision {
                    return Err(stale());
                }
                let staged = r
                    .stages
                    .remove(&(run, ticket.dir, ticket.input_revision))
                    .unwrap_or(Sums::ZERO);
                r.dirs.get_mut(&ticket.dir).unwrap().own = staged;
                // Publishing this listing does not change the inputs reserved by its ticket.
                invalidate(r, ticket.dir, None);
            }
            ScanMessage::DirFinal {
                run,
                ticket,
                sums,
                complete: _,
            } => {
                active(r, run)?;
                let d = r.dirs.get_mut(&ticket.dir).ok_or_else(stale)?;
                if d.dirty_run != run || d.dirty != ticket.input_revision {
                    return Err(stale());
                }
                d.sums = sums;
                d.valid = ticket.input_revision;
            }
            ScanMessage::InvalidateChains {
                run,
                old_parent,
                new_parent,
            } => {
                active(r, run)?;
                *next_rev += 1;
                invalidate(r, old_parent, Some(*next_rev));
                invalidate(r, new_parent, Some(*next_rev));
                r.repairing = true;
                r.sweep = false;
            }
        }
        Ok(())
    }
    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
    ) -> EngineResult<()> {
        let mut s = self.state.lock().unwrap();
        let r = s.roots.get_mut(root).ok_or_else(stale)?;
        active(r, run)?;
        r.active = None;
        r.sweep = complete && matches!(scope, RunScope::FullRoot);
        r.repairing = !complete;
        r.stages.clear();
        Ok(())
    }
    fn fence_root(&self, root: &RootId) -> EngineResult<()> {
        let mut s = self.state.lock().unwrap();
        let r = s.roots.entry(root.clone()).or_default();
        r.fenced = true;
        r.sweep = false;
        r.active = None;
        r.stages.clear();
        Ok(())
    }
}
