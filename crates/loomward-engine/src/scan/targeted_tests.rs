//! #782: target preloading obeys the arena reservation that normal traversal checks.
use super::*;
use crate::budgets::{ByteBudget, SCAN_BYTES};

/// Every directory is empty and reopenable by its numeric ID; listings are counted.
struct Flat(AtomicU64);
fn flat_identity(id: u64) -> OpenedIdentity {
    OpenedIdentity {
        id: Some(FileIdObs::Id64(id)),
        basis: IdBasis::PostOpen,
        volume_serial: Some(7),
        volume_guid: None,
        attributes: 0x10,
        reparse_tag: None,
    }
}
impl DirSource for Flat {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, _: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Ok((1, flat_identity(1)))
    }
    fn can_reopen(&self, _: &u64, _: OpenedIdentity) -> bool {
        true
    }
    fn reopen(
        &self,
        _: &u64,
        _: OpenedIdentity,
        identity: OpenedIdentity,
    ) -> Result<(u64, OpenedIdentity), SourceError> {
        match identity.id {
            Some(FileIdObs::Id64(id)) => Ok((id, flat_identity(id))),
            _ => Err(SourceError::IdentityUnverified),
        }
    }
    fn list(&self, _: &u64, _: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        self.0.fetch_add(1, Ordering::Relaxed);
        ListOutcome::Complete
    }
    fn open_child(&self, _: &u64, _: &RawEntry<'_>) -> Result<(u64, OpenedIdentity), SourceError> {
        Err(SourceError::Unsupported)
    }
}

/// A memory sink that names every requested directory as a root child reopenable by its ID.
struct Targets(MemorySink);
impl ScanSink for Targets {
    fn recover_interrupted(&self) -> EngineResult<()> {
        self.0.recover_interrupted()
    }
    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> EngineResult<()> {
        self.0.begin_run(root, run, scope)
    }
    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> EngineResult<ListingTicket> {
        self.0.prepare_listing(root, run, parent, name, identity)
    }
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> EngineResult<ListingTicket> {
        self.0.refresh_listing(root, run, ticket)
    }
    fn prepare_target(
        &self,
        _: &RootId,
        _: u64,
        dir: u64,
    ) -> EngineResult<Option<TargetDirectory>> {
        Ok(Some(TargetDirectory {
            ticket: ListingTicket {
                dir,
                input_revision: 1,
                old_parent: None,
            },
            parent: Some(0),
            id: FileIdObs::Id64(dir),
            depth: 1,
        }))
    }
    fn consume(&self, root: &RootId, message: ScanMessage) -> EngineResult<()> {
        // Named directories are synthetic tickets the memory sink never issued.
        match &message {
            ScanMessage::DirListing { ticket, .. }
            | ScanMessage::ListingDone { ticket, .. }
            | ScanMessage::DirFinal { ticket, .. }
                if ticket.dir >= 100 =>
            {
                Ok(())
            }
            _ => self.0.consume(root, message),
        }
    }
    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
    ) -> EngineResult<()> {
        self.0.finish_run(root, run, scope, complete)
    }
    fn fence_root(&self, root: &RootId) -> EngineResult<()> {
        self.0.fence_root(root)
    }
}

#[test]
fn target_preloading_over_the_arena_reservation_falls_back_to_the_whole_root() {
    // Enough targets to pass a slot count derived from node bytes alone, but not the combined
    // node-plus-handle reservation every held directory costs.
    let arena = SCAN_BYTES / 4;
    let node = std::mem::size_of::<Slot<u64>>() + 128;
    let handle = std::mem::size_of::<Task<u64>>() + std::mem::size_of::<u64>() + 16;
    let count = (arena / node - 2) as u64;
    assert!((count as usize + 1) * (node + handle) > arena);
    let source = Flat(AtomicU64::new(0));
    let root = GrantedRoot::new(
        RootId::new("rt_782_arena").unwrap(),
        std::path::PathBuf::from("synthetic"),
        loomward_protocol::DatasetClass::Synthetic,
        flat_identity(1),
    );
    let report = run_scan(
        &source,
        &root,
        1,
        Arc::new(Targets(MemorySink::default())),
        Arc::new(ByteBudget::new(SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            workers: 1,
            scope: RunScope::Targeted((100..100 + count).collect()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        source.0.load(Ordering::Relaxed),
        1,
        "the over-budget batch must fall back to listing the root alone"
    );
    assert!(!report.complete);
}

#[test]
fn targets_within_the_reservation_list_only_themselves() {
    let source = Flat(AtomicU64::new(0));
    let root = GrantedRoot::new(
        RootId::new("rt_782_small").unwrap(),
        std::path::PathBuf::from("synthetic"),
        loomward_protocol::DatasetClass::Synthetic,
        flat_identity(1),
    );
    let _ = run_scan(
        &source,
        &root,
        1,
        Arc::new(Targets(MemorySink::default())),
        Arc::new(ByteBudget::new(SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            workers: 1,
            scope: RunScope::Targeted(vec![100, 101, 102]),
            ..Default::default()
        },
    );
    // Three named directories, and never the root.
    assert_eq!(source.0.load(Ordering::Relaxed), 3);
}
