//! The catalogue-backed scan writer: the engine's [`ScanSink`] over `loomward-catalog`'s single
//! writer (docs/41 section 7, errata #117).
//!
//! Every engine call becomes catalogue writer commands, and each one waits for its receipt, which
//! the writer sends only after COMMIT: the engine never sees a publication confirmed early.
//! Listings always take the chunked form (`StageChunk` then `ListingDone`), never the one-shot
//! `DirListing`, so a rejected chunk is fenced by the writer and its listing publishes partial.
//!
//! - **N1.** Tickets carry an adapter input revision per directory. A chunk, publication or final
//!   whose revision is no longer current is rejected `listing_revision_changed`, so the engine
//!   relists it; a refresh first publishes any chunks its obsolete listing staged, upsert-only.
//! - **Finals.** `DirFinal` is checked against N1 but stamps nothing: subtree sums come only from
//!   the writer's checked rollup at `EndRun` (completed or repair), derived from committed rows, so
//!   an engine aggregate can never be stamped over newer rows.
//! - **N2 and absence.** Only a complete `ListingDone` reaches the writer as complete; every other
//!   outcome (outside scope included) is upsert-only. Absence follows the writer's identity-first
//!   matching: an observation carries a file ID only when the engine found it eligible, otherwise
//!   it matches by name. Excluded entries (reparse, offline, recall, state directory, depth limit)
//!   are never published, so they never count as present; they are counted as skipped.
//! - **Partial #8.** The writer reparents a directory inside the new parent's publication and
//!   dirties both ancestor chains in that transaction, cancelled run or not. A child ticket is
//!   resolved after that publication, so no ticket carries an `old_parent`.
//! - **Revocation.** `fence_root` refuses every later call for the revoked grant; the writer
//!   itself already refuses any command for a run whose grant is no longer active.
//! - **Watching.** Not supported (`watch_dirty` refuses), so the engine scans this sink unwatched.

use crate::{db, now_ns, Inner};
use loomward_catalog::{
    Error as CatalogError, ListingOutcome, Observation, WriteCommand, WriteReply,
};
use loomward_engine::scan::source::{
    FileIdObs, IdBasis, IncompleteReason, ListOutcome, OpenedIdentity,
};
use loomward_engine::scan::{Entry, ListingTicket, RunScope, ScanMessage, ScanSink};
use loomward_engine::{EngineError, EngineResult};
use loomward_protocol::{EventName, RootId};
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

const NAME_LOSSY: u32 = 1 << 9;
const IDENTITY_UNAVAILABLE: u32 = 1 << 8;
const ENUMERATION_ERROR: u32 = 1 << 7;
const ACCESS_DENIED: u32 = 1 << 6;
/// `tree.invalidated` is at most 2/s per root (docs/41 section 5.2).
const EVENT_INTERVAL: Duration = Duration::from_millis(500);

/// One granted root's writer state.
struct Root {
    grant: i64,
    root_dir: i64,
    /// Revoked grant: every later call is refused until a new grant is bound.
    fenced: Option<i64>,
    /// `(engine run, catalogue run)` while a run is active.
    run: Option<(u64, i64)>,
    dirs: HashMap<u64, Listing>,
    last_event: Option<Instant>,
}

/// The listing a directory's current ticket reserves.
#[derive(Default)]
struct Listing {
    revision: u64,
    seq: i64,
    staged: bool,
    skipped: u64,
    errors: u64,
}

#[derive(Default)]
struct State {
    next_revision: u64,
    roots: HashMap<RootId, Root>,
}

pub(crate) struct CatalogScanSink {
    inner: Weak<Inner>,
    state: Mutex<State>,
    /// Read-only lookups of rows the writer committed; never written.
    reader: Mutex<rusqlite::Connection>,
    #[cfg(test)]
    pub(crate) gate: crate::tests::Gate,
}

fn stale() -> EngineError {
    EngineError::StaleGeneration {
        message: "obsolete scan writer message".into(),
        detail: loomward_protocol::Detail::new(std::collections::BTreeMap::from([(
            "reason".to_string(),
            json!("listing_revision_changed"),
        )]))
        .ok(),
    }
}

fn revoked() -> EngineError {
    EngineError::PermissionDenied {
        message: "root revoked".into(),
    }
}

fn internal(message: &str) -> EngineError {
    EngineError::Internal {
        message: message.into(),
        detail: None,
    }
}

/// Writer refusals of a listing step. A grant that is no longer active is a revocation; a
/// rejected observation or an unknown directory is an obsolete listing that the engine relists
/// (and reports partial if it never settles); anything else stops the scan.
fn listing_error(e: CatalogError) -> EngineError {
    match e {
        CatalogError::Invalid("inactive run or grant") => revoked(),
        CatalogError::Invalid(_) | CatalogError::NotFound | CatalogError::StaleGeneration => {
            stale()
        }
        CatalogError::Cancelled => EngineError::Cancelled {
            message: "catalogue writer cancelled".into(),
        },
        _ => internal("catalogue writer failed"),
    }
}

fn id_bytes(id: FileIdObs) -> Option<Vec<u8>> {
    match id.nonzero()? {
        FileIdObs::Id128(id) => Some(id.to_vec()),
        FileIdObs::Id64(id) => Some(id.to_le_bytes().to_vec()),
    }
}

/// Exact name: UTF-8 when the UTF-16 is well formed, otherwise a lossy display name plus the raw
/// UTF-16 bytes, so an unpaired surrogate still matches only itself.
fn name(raw: &[u16]) -> (String, Option<Vec<u8>>) {
    match String::from_utf16(raw) {
        Ok(s) => (s, None),
        Err(_) => (
            String::from_utf16_lossy(raw),
            Some(raw.iter().flat_map(|c| c.to_le_bytes()).collect()),
        ),
    }
}

/// One listed entry as a catalogue observation; `None` for an excluded entry.
fn observation(e: &Entry) -> Option<Observation> {
    if e.excluded {
        return None;
    }
    let (text, raw) = name(&e.name);
    let file_id = e.file_id.filter(|_| e.identity_eligible).and_then(id_bytes);
    let directory = e.attributes & 0x10 != 0;
    let extension = (!directory)
        .then(|| text.rsplit_once('.'))
        .flatten()
        .map(|(stem, ext)| (stem, ext.to_lowercase()))
        .filter(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.chars().count() <= 32)
        .map(|(_, ext)| ext);
    let mut flags = 0;
    if raw.is_some() {
        flags |= NAME_LOSSY;
    }
    if file_id.is_none() {
        flags |= IDENTITY_UNAVAILABLE;
    }
    match e.traversal_error {
        Some(loomward_engine::scan::source::SourceError::AccessDenied) => flags |= ACCESS_DENIED,
        Some(_) => flags |= ENUMERATION_ERROR,
        None => {}
    }
    Some(Observation {
        name: text,
        name_utf16: raw,
        id_basis: if file_id.is_some() { "listed" } else { "none" }.into(),
        file_id,
        logical: e.logical,
        allocated: e.allocated,
        // Family classification belongs to a later lane: an extension is only "other" here.
        family: if extension.is_some() { "other" } else { "none" }.into(),
        extension,
        created_ft: e.creation,
        modified_ft: e.last_write,
        changed_ft: e.change,
        accessed_ft: e.last_access,
        attrs: e.attributes,
        reparse_tag: e.reparse_tag,
        flags,
    })
}

fn outcome(o: ListOutcome) -> ListingOutcome {
    match o {
        ListOutcome::Complete => ListingOutcome::Complete,
        ListOutcome::Incomplete(reason) => ListingOutcome::Incomplete(
            match reason {
                IncompleteReason::Cancelled => "cancelled",
                IncompleteReason::AccessDenied => "denied",
                IncompleteReason::OutsideScope => "stale",
                IncompleteReason::Io(_) | IncompleteReason::Malformed | IncompleteReason::Limit => {
                    "partial"
                }
            }
            .into(),
        ),
    }
}

impl CatalogScanSink {
    pub(crate) fn new(inner: Weak<Inner>, reader: rusqlite::Connection) -> Self {
        Self {
            inner,
            state: Mutex::new(State::default()),
            reader: Mutex::new(reader),
            #[cfg(test)]
            gate: Default::default(),
        }
    }

    /// Binds a wire root to its active grant before a scan; a new grant lifts an old fence.
    pub(crate) fn bind(&self, root: &RootId, grant: i64, root_dir: i64) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let r = s.roots.entry(root.clone()).or_insert(Root {
            grant,
            root_dir,
            fenced: None,
            run: None,
            dirs: HashMap::new(),
            last_event: None,
        });
        r.grant = grant;
        r.root_dir = root_dir;
    }

    // ponytail: one command per receipt, so the writer never batches a scan's listings (the
    // 159-directory lab test takes about 6 s here); pipeline chunk receipts and wait only at
    // ListingDone when scan throughput matters (#148).
    fn call(&self, command: WriteCommand) -> loomward_catalog::Result<WriteReply> {
        let inner = self.inner.upgrade().ok_or(CatalogError::Closed)?;
        let reply = inner.catalog.writer().call(command);
        reply
    }

    /// The root's state for an active run `run`, refused once fenced.
    fn active<'a>(s: &'a mut State, root: &RootId, run: u64) -> EngineResult<&'a mut Root> {
        let r = s.roots.get_mut(root).ok_or_else(stale)?;
        if r.fenced == Some(r.grant) {
            return Err(revoked());
        }
        if r.run.map(|(engine, _)| engine) != Some(run) {
            return Err(EngineError::StaleGeneration {
                message: "obsolete scan run".into(),
                detail: None,
            });
        }
        Ok(r)
    }

    /// The listing a ticket reserved, if that reservation is still current (N1).
    fn current(r: &mut Root, ticket: ListingTicket) -> EngineResult<&mut Listing> {
        r.dirs
            .get_mut(&ticket.dir)
            .filter(|l| l.revision == ticket.input_revision)
            .ok_or_else(stale)
    }

    /// The child row the parent's publication committed: by native identity first, else by exact
    /// name among identity-less children. Hidden tombstones never match.
    fn child_row(
        &self,
        root_dir: i64,
        parent: i64,
        name: &[u16],
        id: Option<Vec<u8>>,
    ) -> EngineResult<Option<i64>> {
        let conn = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        let fail = |_| internal("catalogue read failed");
        if let Some(id) = id {
            let row = conn
                .query_row(
                    "SELECT id FROM main.dir WHERE root_id=(SELECT root_id FROM main.dir WHERE id=?1)
                     AND file_id=?2 AND parent_id=?3 AND listing_state!='absent_pending'",
                    params![root_dir, id, parent],
                    |r| r.get(0),
                )
                .optional()
                .map_err(fail)?;
            if row.is_some() {
                return Ok(row);
            }
        }
        let (text, raw) = self::name(name);
        conn.query_row(
            "SELECT id FROM main.dir WHERE parent_id=?1 AND name=?2 AND name_utf16 IS ?3
             AND file_id IS NULL AND listing_state!='absent_pending'",
            params![parent, text, raw],
            |r| r.get(0),
        )
        .optional()
        .map_err(fail)
    }

    /// Throttled `tree.invalidated` after a committed publication; `force` for the run's end.
    fn invalidated(&self, root: &RootId, force: bool) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(r) = s.roots.get_mut(root) else {
                return;
            };
            if !force && r.last_event.is_some_and(|t| t.elapsed() < EVENT_INTERVAL) {
                return;
            }
            r.last_event = Some(Instant::now());
        }
        let generation = inner
            .root_row(root.as_str())
            .ok()
            .and_then(|row| db::root_generation(&inner.db(), row).ok().flatten());
        let Ok((catalog_rev, state_rev)) = inner.revisions() else {
            return;
        };
        inner.publish(
            EventName::TreeInvalidated,
            json!({
                "root_id": root,
                "generation": generation.map(|g| g.to_string()),
                "scope": "all",
                "node_ids": [],
            }),
            Some(catalog_rev),
            Some(state_rev),
        );
    }
}

impl ScanSink for CatalogScanSink {
    /// The catalogue already failed interrupted runs and marked their roots repairing when it
    /// opened, before the service could serve any slice.
    fn recover_interrupted(&self) -> EngineResult<()> {
        Ok(())
    }

    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> EngineResult<()> {
        let grant = {
            let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let r = s.roots.get(root).ok_or_else(revoked)?;
            if r.fenced == Some(r.grant) {
                return Err(revoked());
            }
            if r.run.is_some() {
                return Err(EngineError::Busy {
                    message: "root writer busy".into(),
                    detail: None,
                });
            }
            r.grant
        };
        let reply = self
            .call(WriteCommand::BeginRun {
                grant_id: grant,
                mode: match scope {
                    RunScope::FullRoot => "full",
                    RunScope::Targeted(_) => "targeted",
                }
                .into(),
                strategy: "native".into(),
                started_at_ns: now_ns(),
            })
            .map_err(|e| match e {
                CatalogError::NotFound => revoked(),
                CatalogError::Invalid("run already active") => EngineError::Busy {
                    message: "root writer busy".into(),
                    detail: None,
                },
                _ => internal("catalogue writer failed"),
            })?;
        let WriteReply::Run(catalog_run) = reply else {
            return Err(internal("unexpected writer reply"));
        };
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let r = s.roots.get_mut(root).ok_or_else(revoked)?;
        if r.fenced == Some(r.grant) {
            // Revoked while the run began: the writer has already cancelled it.
            return Err(revoked());
        }
        r.run = Some((run, catalog_run));
        r.dirs.clear();
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
        #[cfg(test)]
        self.gate.pass(parent.is_some());
        let root_dir = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Self::active(&mut s, root, run)?.root_dir
        };
        let dir = match parent {
            None => root_dir,
            Some(parent) => {
                let id = identity
                    .id
                    .filter(|_| matches!(identity.basis, IdBasis::Listed | IdBasis::PostOpen))
                    .and_then(id_bytes);
                // Unknown: the parent's publication was rejected or never named this child.
                self.child_row(root_dir, parent as i64, name, id)?
                    .ok_or_else(stale)?
            }
        } as u64;
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.next_revision += 1;
        let revision = s.next_revision;
        let r = Self::active(&mut s, root, run)?;
        r.dirs.insert(
            dir,
            Listing {
                revision,
                ..Listing::default()
            },
        );
        Ok(ListingTicket {
            dir,
            input_revision: revision,
            old_parent: None,
        })
    }

    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> EngineResult<ListingTicket> {
        let (catalog_run, staged) = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let r = Self::active(&mut s, root, run)?;
            let catalog_run = r.run.unwrap().1;
            let listing = r.dirs.get_mut(&ticket.dir).ok_or_else(stale)?;
            let staged = std::mem::take(listing);
            (catalog_run, staged)
        };
        if staged.staged {
            // The obsolete listing's accepted chunks publish upsert-only, so the relist starts
            // from an empty stage and nothing it staged can establish absence.
            self.call(WriteCommand::ListingDone {
                run_id: catalog_run,
                dir_id: ticket.dir as i64,
                outcome: ListingOutcome::Incomplete("stale".into()),
                skipped: staged.skipped,
                errors: staged.errors,
            })
            .map_err(listing_error)?;
        }
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.next_revision += 1;
        let revision = s.next_revision;
        let r = Self::active(&mut s, root, run)?;
        r.dirs.insert(
            ticket.dir,
            Listing {
                revision,
                ..Listing::default()
            },
        );
        Ok(ListingTicket {
            input_revision: revision,
            old_parent: None,
            ..ticket
        })
    }

    fn consume(&self, root: &RootId, message: ScanMessage) -> EngineResult<()> {
        match message {
            ScanMessage::DirListing {
                run,
                ticket,
                entries,
            } => {
                let (catalog_run, seq) = {
                    let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let r = Self::active(&mut s, root, run)?;
                    let catalog_run = r.run.unwrap().1;
                    let listing = Self::current(r, ticket)?;
                    for e in &entries {
                        listing.skipped += u64::from(e.excluded);
                        listing.errors += u64::from(e.traversal_error.is_some());
                    }
                    (catalog_run, listing.seq)
                };
                let (mut files, mut dirs) = (Vec::new(), Vec::new());
                for o in entries.iter().filter_map(observation) {
                    if o.attrs & 0x10 != 0 {
                        dirs.push(o);
                    } else {
                        files.push(o);
                    }
                }
                if files.is_empty() && dirs.is_empty() {
                    return Ok(());
                }
                self.call(WriteCommand::StageChunk {
                    run_id: catalog_run,
                    dir_id: ticket.dir as i64,
                    seq,
                    files,
                    dirs,
                })
                .map_err(listing_error)?;
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let r = Self::active(&mut s, root, run)?;
                let listing = Self::current(r, ticket)?;
                listing.seq += 1;
                listing.staged = true;
                Ok(())
            }
            ScanMessage::ListingDone {
                run,
                ticket,
                outcome: done,
            } => {
                let (catalog_run, skipped, errors) = {
                    let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let r = Self::active(&mut s, root, run)?;
                    let catalog_run = r.run.unwrap().1;
                    let listing = Self::current(r, ticket)?;
                    (catalog_run, listing.skipped, listing.errors)
                };
                self.call(WriteCommand::ListingDone {
                    run_id: catalog_run,
                    dir_id: ticket.dir as i64,
                    outcome: outcome(done),
                    skipped,
                    errors,
                })
                .map_err(listing_error)?;
                {
                    let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let r = Self::active(&mut s, root, run)?;
                    if let Ok(listing) = Self::current(r, ticket) {
                        // Published: the stage is empty and its counters are spent.
                        *listing = Listing {
                            revision: listing.revision,
                            ..Listing::default()
                        };
                    }
                }
                self.invalidated(root, false);
                Ok(())
            }
            ScanMessage::DirFinal { run, ticket, .. } => {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let r = Self::active(&mut s, root, run)?;
                Self::current(r, ticket).map(|_| ())
            }
            ScanMessage::InvalidateChains { run, .. } => {
                // Never produced by this sink (no ticket carries old_parent); if it ever arrives,
                // every reserved listing becomes obsolete, conservatively.
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let base = s.next_revision;
                let r = Self::active(&mut s, root, run)?;
                let mut next = base;
                for listing in r.dirs.values_mut() {
                    next += 1;
                    listing.revision = next;
                }
                s.next_revision = next;
                Ok(())
            }
        }
    }

    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        _scope: &RunScope,
        complete: bool,
    ) -> EngineResult<()> {
        let catalog_run = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let r = Self::active(&mut s, root, run)?;
            let catalog_run = r.run.take().unwrap().1;
            r.dirs.clear();
            catalog_run
        };
        // Only an engine-complete run may let the writer finalise absence and the generation;
        // the writer still requires its own full traversal for that.
        self.call(WriteCommand::EndRun {
            run_id: catalog_run,
            state: if complete { "completed" } else { "cancelled" }.into(),
            finished_at_ns: now_ns(),
        })
        .map_err(listing_error)?;
        self.invalidated(root, true);
        Ok(())
    }

    fn fence_root(&self, root: &RootId) -> EngineResult<()> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = s.roots.get_mut(root) {
            r.fenced = Some(r.grant);
            r.run = None;
            r.dirs.clear();
        }
        Ok(())
    }
}
