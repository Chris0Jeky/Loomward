//! Write-ahead journal records (doc 47 section 5, "Write-ahead order and recovery") and their replay.
//!
//! A journal is an append-only, hash-chained sequence of records per operation. Replay applies each
//! record through the section 5 state machine plus the record's own preconditions. Anything that does not
//! replay cleanly (a gap, a chain break, an unreadable or truncated body, an illegal or out-of-order record,
//! a stale epoch, an unknown item or identity) is `Invalid`, which recovery turns into `needs_attention`.
use crate::state::OpState;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};

/// Manifest entry limit (doc 47 section 7, `manifest_limit`; assumption until measured).
pub const MAX_ITEMS: u32 = 200_000;
/// Items created empty per identity commit (doc 47 section 4).
pub const MAX_BATCH: usize = 256;

/// A staged object's identity as journaled: the 128-bit file ID plus its creation time. Cleanup
/// authority comes only from this pair, never from a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectId {
    pub file_id: [u8; 16],
    pub creation_ft: i64,
}

impl ObjectId {
    /// `observed` is this journaled object: the same file ID with the journaled creation time, or with
    /// the planned creation time journaled (`Record::Planned`) before metadata preservation changed it.
    /// Never a name: a name only ever confirms an identity found this way.
    pub fn matches(&self, observed: &ObjectId, planned: Option<i64>) -> bool {
        self.file_id == observed.file_id
            && (observed.creation_ft == self.creation_ft || Some(observed.creation_ft) == planned)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FenceCause {
    GrantRevoked,
    EmergencyStop,
}

/// Doc 47 section 7 codes that the journal records after effects started (item, publish and run level).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    SourceInUse,
    AccessDenied,
    InsufficientSpace,
    StagingInUse,
    PublishInUse,
    SpecialFile,
    SourceChanged,
    CloudPlaceholder,
    ReparsePoint,
    UnsupportedAttribute,
    ExtendedAttributes,
    AlternateStreamsUnsupported,
    TooManyStreams,
    StagingNameTaken,
    VerificationMismatch,
    ChangedAfterVerification,
    GrantRevoked,
    /// The destination parent no longer opens as its pinned identity under its grant's root (at start).
    DestinationChanged,
}

impl Refusal {
    /// Resumable codes leave the run `blocked` with staging kept; every other code is `needs_attention`.
    pub fn resumable(self) -> bool {
        use Refusal::*;
        matches!(
            self,
            SourceInUse | AccessDenied | InsufficientSpace | StagingInUse | PublishInUse
        )
    }

    fn is_item(self) -> bool {
        !matches!(
            self,
            Refusal::PublishInUse
                | Refusal::ChangedAfterVerification
                | Refusal::GrantRevoked
                | Refusal::DestinationChanged
        )
    }
}

/// LW-146: one bounded snapshot before cancelling cleanup changes any item evidence.
/// No paths, names, model output or per-item list; refusal codes are distinct journal facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingDisposition {
    pub staging: ObjectId,
    pub items_total: u32,
    pub items_verified: u32,
    pub items_unverified: u32,
    pub refusal_codes: Vec<Refusal>,
    /// The journal was blocked on a retryable refusal, without a fence, before cancellation.
    pub resumable_before_cancel: bool,
    pub resume_possible: bool,
}

impl StagingDisposition {
    pub fn explanation(&self) -> String {
        let cause = if self.refusal_codes.is_empty() {
            "no refusal was journaled".into()
        } else {
            self.refusal_codes
                .iter()
                .map(|code| {
                    serde_json::to_value(code)
                        .expect("refusal serialises")
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let resume = if self.resumable_before_cancel {
            "The journal marked the stopped run resumable before cancel or abandon."
        } else {
            "No resumable refusal was active at cancellation."
        };
        format!("Cancelled or abandoned; journaled cause: {cause}. {} of {} items verified; {} unverified. {resume} No resume command is available; this cancelled operation cannot resume. Only journaled unpublished staging is eligible for identity-bound removal. The journal, manifest digest, identities, refusal codes and this disposition remain in state.db; staged bytes do not remain after successful removal. Cleanup does not modify the source.", self.items_verified, self.items_total, self.items_unverified)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The approval was consumed: `approved -> preparing`. Binds the manifest digest and item count.
    Started {
        digest: [u8; 32],
        items: u32,
    },
    /// Intent: create the staging folder under this random single-component token.
    StagingIntent {
        token: String,
    },
    /// Outcome: the staging folder's identity, committed before anything is created in it. `preparing -> copying`.
    StagingCreated {
        id: ObjectId,
    },
    /// Intent: create these items empty, under these staging-relative names (`/`-separated).
    CreateBatch {
        items: Vec<(u32, String)>,
    },
    /// Outcome: identities of the items just created, before any byte is written or preallocated.
    Identities {
        items: Vec<(u32, ObjectId)>,
    },
    /// Outcome: items verified by non-cached read-back (may be group-committed).
    Verified {
        items: Vec<u32>,
    },
    ItemFailed {
        item: u32,
        code: Refusal,
    },
    /// Cleanup or redo outcome: the item's journaled object was removed by identity.
    ItemRemoved {
        item: u32,
        id: ObjectId,
    },
    /// Owner or executor edges without a dedicated record (resume, cancel, staged, blocked, ...).
    Transition {
        to: OpState,
        code: Option<Refusal>,
    },
    /// A revocation or emergency stop committed and raised the fence epoch to this entry's epoch.
    Fenced {
        cause: FenceCause,
    },
    /// A cause observed while nonterminal, recorded after another Store committed first (#738).
    /// Metadata only: preserves the terminal state, fence epoch, identities and effect restrictions.
    /// Older record decoders refuse this kind; they must not silently skip it on rollback.
    RevokedAfterPublish {
        cause: FenceCause,
    },
    /// Intent: rename staging to this final single-component name without replace. `staged -> publishing`.
    PublishIntent {
        final_name: String,
    },
    /// The pending publish intent is withdrawn; no rename will be issued. `-> cancelling`.
    PublishWithdrawn,
    /// Post-publish confirmation passed. `publishing -> committed`.
    Committed,
    /// Cleanup outcome: the empty staging folder was removed by identity.
    StagingRemoved {
        id: ObjectId,
    },
    /// Durable explanation facts before any cancellation cleanup; retained after item removal.
    StagingDisposition {
        facts: StagingDisposition,
    },
    /// Intent (lane C5, #506): set these creation times next, on items (and on the staging folder when
    /// `staging` is set). Creation-time preservation changes the identity record, so recovery must accept
    /// the journaled ID with either its original or its planned creation time.
    Planned {
        items: Vec<(u32, i64)>,
        staging: Option<i64>,
    },
    /// Cleanup outcome under the docs/47 Q1 default (`loomward-copy-staging-removal` off): nothing was
    /// removed; the journaled staging folder, with whatever it holds, is left for the owner. `-> cancelled`
    /// may follow. Loomward keeps no cleanup authority over it afterwards.
    StagingLeft {
        id: ObjectId,
    },
}

impl Record {
    /// Doc 47 section 5 rule 6: these records are valid under an older epoch (a fence never strands them).
    fn epoch_exempt(&self) -> bool {
        matches!(
            self,
            Record::Committed
                | Record::PublishWithdrawn
                | Record::ItemRemoved { .. }
                | Record::StagingRemoved { .. }
                | Record::StagingDisposition { .. }
                | Record::StagingLeft { .. }
                | Record::Transition {
                    to: OpState::Cancelling | OpState::Cancelled,
                    ..
                }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub epoch: u64,
    #[serde(flatten)]
    pub record: Record,
}

/// One stored journal row. `chain` = SHA-256(previous chain, op id, seq, body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub seq: u64,
    pub body: String,
    pub chain: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid {
    pub seq: u64,
    pub why: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Item {
    pub name: Option<String>,
    pub id: Option<ObjectId>,
    pub verified: bool,
    pub mismatches: u8,
    /// Creation time journaled by `Record::Planned` for the current identity.
    pub planned_ft: Option<i64>,
}

/// An item as recovery names it: always a journaled identity under a journaled staging-relative name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedItem {
    pub item: u32,
    pub name: String,
    pub id: ObjectId,
    pub planned_ft: Option<i64>,
}

/// The state a journal replays to.
#[derive(Debug, Clone)]
pub struct Replay {
    pub op: u64,
    pub state: OpState,
    pub epoch: u64,
    pub fenced: bool,
    /// Set on entering `cancelling` and never cleared: Abandon is then the only exit (doc 47 section 5).
    pub cancelled: bool,
    pub digest: [u8; 32],
    pub items: Vec<Item>,
    /// Every staging token ever intended, in order; only the last can carry the staging identity.
    pub staging_tokens: Vec<String>,
    pub staging: Option<ObjectId>,
    /// Creation time journaled by `Record::Planned` for the staging folder.
    pub staging_planned: Option<i64>,
    pub staging_removed: bool,
    /// `Record::StagingLeft`: the staging folder was left for the owner (Q1 default).
    pub staging_left: bool,
    pub disposition: Option<StagingDisposition>,
    pub open_batch: Vec<u32>,
    /// Final name of a publish intent that is neither withdrawn nor superseded.
    pub publish: Option<String>,
    pub seq: u64,
    pub chain: [u8; 32],
    names: HashMap<String, u32>,
    seen: HashSet<ObjectId>,
    fence_causes: Vec<FenceCause>,
    refusal_codes: Vec<Refusal>,
    resumable_before_cancel: bool,
}

pub(crate) fn component_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 1024
        && s != "."
        && s != ".."
        && !s
            .chars()
            .any(|c| c < ' ' || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
}

/// A staging-relative name: one or more valid components joined by `/`; never absolute, never `..`.
pub(crate) fn relative_ok(s: &str) -> bool {
    s.split('/').all(component_ok)
}

fn chain(prev: &[u8; 32], op: u64, seq: u64, body: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(prev);
    h.update(op.to_le_bytes());
    h.update(seq.to_le_bytes());
    h.update(body.as_bytes());
    h.finalize().into()
}

impl Replay {
    /// An operation before its `started` record.
    pub fn new(op: u64) -> Self {
        Replay {
            op,
            state: OpState::Approved,
            epoch: 0,
            fenced: false,
            cancelled: false,
            digest: [0; 32],
            items: Vec::new(),
            staging_tokens: Vec::new(),
            staging: None,
            staging_planned: None,
            staging_removed: false,
            staging_left: false,
            disposition: None,
            open_batch: Vec::new(),
            publish: None,
            seq: 0,
            chain: [0; 32],
            names: HashMap::new(),
            seen: HashSet::new(),
            fence_causes: Vec::new(),
            refusal_codes: Vec::new(),
            resumable_before_cancel: false,
        }
    }

    pub fn from_rows(op: u64, rows: &[Row]) -> Result<Replay, Invalid> {
        if rows.is_empty() {
            return Err(Invalid {
                seq: 0,
                why: "empty journal",
            });
        }
        let mut replay = Replay::new(op);
        for row in rows {
            replay.apply(row)?;
        }
        Ok(replay)
    }

    /// The row that would append `entry` next. Applying it is still what validates it.
    pub fn row_for(&self, entry: &Entry) -> Row {
        let body = serde_json::to_string(entry).expect("journal records serialise");
        let seq = self.seq + 1;
        Row {
            seq,
            chain: chain(&self.chain, self.op, seq, &body).to_vec(),
            body,
        }
    }

    /// Apply one row. On `Err` the replay is poisoned and must be discarded.
    pub fn apply(&mut self, row: &Row) -> Result<(), Invalid> {
        let seq = self.seq + 1;
        let fail = |why| Err(Invalid { seq, why });
        if row.seq != seq {
            return fail("sequence gap, duplicate or reorder");
        }
        if row.chain.as_slice() != chain(&self.chain, self.op, seq, &row.body) {
            return fail("chain mismatch: corrupt, truncated or foreign record");
        }
        let Ok(entry) = serde_json::from_str::<Entry>(&row.body) else {
            return fail("unreadable record");
        };
        self.apply_entry(entry).or_else(fail)?;
        self.seq = seq;
        self.chain.copy_from_slice(&row.chain);
        Ok(())
    }

    fn item(&mut self, i: u32) -> Result<&mut Item, &'static str> {
        self.items.get_mut(i as usize).ok_or("unknown item")
    }

    fn fresh(&mut self, id: ObjectId) -> Result<(), &'static str> {
        if self.seen.insert(id) {
            Ok(())
        } else {
            Err("identity journaled twice")
        }
    }

    fn go(&mut self, to: OpState) -> Result<(), &'static str> {
        if to == OpState::Cancelling && !self.cancelled {
            self.resumable_before_cancel = self.state == OpState::Blocked && !self.fenced;
        }
        self.state = self
            .state
            .transition(to)
            .map_err(|_| "illegal state transition")?;
        self.cancelled |= to == OpState::Cancelling;
        Ok(())
    }

    fn apply_entry(&mut self, Entry { epoch, record }: Entry) -> Result<(), &'static str> {
        use OpState::*;
        if self.seq == 0 {
            let Record::Started { digest, items } = record else {
                return Err("journal does not begin with started");
            };
            if items == 0 || items > MAX_ITEMS {
                return Err("item count out of range");
            }
            self.go(Preparing)?;
            self.epoch = epoch;
            self.digest = digest;
            self.items = vec![Item::default(); items as usize];
            return Ok(());
        }
        // Fence epochs (doc 47 section 5, revocation rules 1 and 6).
        match &record {
            Record::Fenced { cause } => {
                if self.state.is_terminal() {
                    return Err("fence on a finished operation");
                }
                if epoch <= self.epoch {
                    return Err("fence does not raise the epoch");
                }
                self.epoch = epoch;
                self.fenced = true;
                self.remember_refusal(Refusal::GrantRevoked);
                if !self.fence_causes.contains(cause) {
                    self.fence_causes.push(*cause);
                }
                return Ok(());
            }
            r if r.epoch_exempt() => {
                if epoch > self.epoch {
                    return Err("epoch ahead of the fence");
                }
            }
            _ if epoch != self.epoch => return Err("stale epoch"),
            _ => {}
        }
        let advancing = match &record {
            Record::StagingIntent { .. }
            | Record::StagingCreated { .. }
            | Record::CreateBatch { .. }
            | Record::Identities { .. }
            | Record::Verified { .. }
            | Record::ItemFailed { .. }
            | Record::Planned { .. }
            | Record::PublishIntent { .. } => true,
            Record::Transition { to, .. } => matches!(to, Copying | Staged | Blocked),
            _ => false,
        };
        if self.fenced && advancing {
            return Err("record advances a fenced run");
        }
        if self.cancelled && advancing {
            return Err("record advances a cancelled run");
        }
        match record {
            Record::Started { .. } => return Err("started twice"),
            Record::Fenced { .. } => unreachable!(),
            Record::RevokedAfterPublish { cause } => {
                if self.state != Committed || self.publish.is_none() || self.staging.is_none() {
                    return Err("terminal annotation without a committed publish identity");
                }
                if self.has_fence_cause(cause) {
                    return Err("revocation cause already journaled");
                }
                self.fence_causes.push(cause);
            }
            Record::StagingIntent { token } => {
                if self.state != Preparing || self.staging.is_some() {
                    return Err("staging intent outside preparing");
                }
                if !component_ok(&token) || self.staging_tokens.contains(&token) {
                    return Err("invalid or reused staging token");
                }
                self.staging_tokens.push(token);
            }
            Record::StagingCreated { id } => {
                if self.state != Preparing || self.staging_tokens.is_empty() {
                    return Err("staging identity without intent");
                }
                self.fresh(id)?;
                self.staging = Some(id);
                self.go(Copying)?;
            }
            Record::CreateBatch { items } => {
                if self.state != Copying {
                    return Err("create batch outside copying");
                }
                if items.is_empty() || items.len() > MAX_BATCH {
                    return Err("create batch size out of range");
                }
                // A new batch only once the last one is closed; otherwise only the same batch again (a retry).
                let mut ids: Vec<u32> = items.iter().map(|(i, _)| *i).collect();
                let mut open = self.open_batch.clone();
                ids.sort_unstable();
                open.sort_unstable();
                if !open.is_empty() && ids != open {
                    return Err("create batch while another batch is open");
                }
                let mut batch = Vec::with_capacity(items.len());
                for (i, name) in items {
                    if !relative_ok(&name) {
                        return Err("invalid staging-relative name");
                    }
                    if batch.contains(&i) {
                        return Err("item twice in one batch");
                    }
                    if let Some(&owner) = self.names.get(&name) {
                        if owner != i {
                            return Err("name belongs to another item");
                        }
                    }
                    let item = self.item(i)?;
                    if item.id.is_some() || item.verified {
                        return Err("create intent for an item that exists");
                    }
                    if item.name.as_ref().is_some_and(|n| *n != name) {
                        return Err("item renamed between attempts");
                    }
                    item.name = Some(name.clone());
                    self.names.insert(name, i);
                    batch.push(i);
                }
                self.open_batch = batch;
            }
            Record::Identities { items } => {
                // An outcome of creates already made: also after the item failure that interrupted the batch.
                if !matches!(self.state, Copying | Blocked | NeedsAttention) || items.is_empty() {
                    return Err("identity commit outside copying");
                }
                let mut covered: Vec<u32> = items.iter().map(|(i, _)| *i).collect();
                let mut open = std::mem::take(&mut self.open_batch);
                covered.sort_unstable();
                open.sort_unstable();
                // After an item failure the executor stops creating: the commit covers the items it
                // created, and the never-attempted rest leave the batch with their names journaled and no
                // identity (never adopted, reported if something appears there). Otherwise: exactly.
                let partial =
                    self.state != Copying && covered.iter().all(|i| open.binary_search(i).is_ok());
                if covered.windows(2).any(|w| w[0] == w[1]) || (covered != open && !partial) {
                    return Err("identity commit does not cover the open batch exactly");
                }
                for (i, id) in items {
                    self.fresh(id)?;
                    self.item(i)?.id = Some(id);
                }
            }
            Record::Verified { items } => {
                if self.state != Copying || items.is_empty() {
                    return Err("verified outside copying");
                }
                for i in items {
                    let item = self.item(i)?;
                    if item.id.is_none() || item.verified {
                        return Err("verified without identity, or twice");
                    }
                    item.verified = true;
                }
            }
            Record::ItemFailed { item: i, code } => {
                if self.state != Copying || !code.is_item() {
                    return Err("item failure outside copying");
                }
                self.remember_refusal(code);
                // A create that failed inside the open batch leaves the batch; the rest still commit.
                self.open_batch.retain(|&x| x != i);
                let item = self.item(i)?;
                if item.verified {
                    return Err("failure of a verified item");
                }
                if code == Refusal::VerificationMismatch {
                    if item.id.is_none() {
                        return Err("mismatch without identity");
                    }
                    item.mismatches += 1;
                    if item.mismatches >= 2 {
                        self.go(NeedsAttention)?;
                    }
                } else if code.resumable() {
                    self.go(Blocked)?;
                } else {
                    self.go(NeedsAttention)?;
                }
            }
            Record::ItemRemoved { item: i, id } => {
                let cancelling = self.state == Cancelling;
                if !(cancelling || self.state == Copying) || self.publish.is_some() {
                    return Err("removal outside copying or cancelling");
                }
                if self.staging_left {
                    return Err("removal after the staging was left to the owner");
                }
                let item = self.item(i)?;
                if item.id != Some(id) {
                    return Err("removal of an identity the item does not hold");
                }
                if item.verified && !cancelling {
                    return Err("redo of a verified item");
                }
                item.id = None;
                item.verified = false;
                item.planned_ft = None;
            }
            Record::Planned { items, staging } => {
                if self.state != Copying || (items.is_empty() && staging.is_none()) {
                    return Err("planned metadata outside copying");
                }
                if staging.is_some() && self.staging.is_none() {
                    return Err("planned staging metadata without a staging identity");
                }
                for (i, ft) in items {
                    let item = self.item(i)?;
                    if item.id.is_none() || item.verified {
                        return Err("planned metadata without identity, or after verification");
                    }
                    item.planned_ft = Some(ft);
                }
                if staging.is_some() {
                    self.staging_planned = staging;
                }
            }
            Record::StagingLeft { id } => {
                if self.state != Cancelling || self.publish.is_some() {
                    return Err("staging left outside cancelling");
                }
                if self.staging != Some(id) || self.staging_removed || self.staging_left {
                    return Err("staging left of an unjournaled identity");
                }
                self.staging_left = true;
            }
            Record::StagingDisposition { facts } => {
                if self.state != Cancelling
                    || self.publish.is_some()
                    || self.staging_left
                    || self.staging_removed
                    || self.disposition.is_some()
                    || self.staging_disposition().as_ref() != Some(&facts)
                {
                    return Err("disposition does not match unpublished cancelling staging");
                }
                self.disposition = Some(facts);
            }
            Record::Transition { to, code } => {
                match to {
                    Preparing | Publishing | Committed => {
                        return Err("transition needs its dedicated record")
                    }
                    Copying if !matches!(self.state, Blocked | NeedsAttention) => {
                        return Err("transition needs its dedicated record")
                    }
                    Staged => {
                        let done = self.items.iter().all(|i| i.verified);
                        if !done || !self.open_batch.is_empty() {
                            return Err("staged before every item verified");
                        }
                    }
                    Blocked if !code.is_some_and(Refusal::resumable) => {
                        return Err("blocked without a resumable code")
                    }
                    Cancelling if self.publish.is_some() => {
                        return Err("cancel with a publish intent not withdrawn")
                    }
                    Cancelled if !self.staging_left => {
                        let held = self.items.iter().any(|i| i.id.is_some());
                        if held || (self.staging.is_some() && !self.staging_removed) {
                            return Err("cancelled with journaled staging left");
                        }
                    }
                    _ => {}
                }
                self.go(to)?;
                if let Some(code) = code {
                    self.remember_refusal(code);
                }
            }
            Record::PublishIntent { final_name } => {
                if !component_ok(&final_name) || self.staging_tokens.contains(&final_name) {
                    return Err("invalid final name");
                }
                if self.state != Staged {
                    return Err("publish intent outside staged");
                }
                self.go(Publishing)?;
                self.publish = Some(final_name);
            }
            Record::PublishWithdrawn => {
                if self.publish.take().is_none() {
                    return Err("withdrawal without a publish intent");
                }
                self.go(Cancelling)?;
            }
            Record::Committed => {
                if self.state != Publishing || self.publish.is_none() {
                    return Err("committed outside publishing");
                }
                self.go(Committed)?;
            }
            Record::StagingRemoved { id } => {
                if self.state != Cancelling || self.publish.is_some() {
                    return Err("staging removal outside cancelling");
                }
                if self.staging != Some(id) || self.staging_removed || self.staging_left {
                    return Err("staging removal of an unjournaled identity");
                }
                if self.items.iter().any(|i| i.id.is_some()) {
                    return Err("staging removed before its items");
                }
                self.staging_removed = true;
            }
        }
        Ok(())
    }

    fn remember_refusal(&mut self, code: Refusal) {
        if !self.refusal_codes.contains(&code) {
            self.refusal_codes.push(code);
        }
    }

    /// The immutable pre-removal counts, or the facts a first disposition must match exactly.
    pub fn staging_disposition(&self) -> Option<StagingDisposition> {
        if let Some(facts) = &self.disposition {
            return Some(facts.clone());
        }
        let items_verified = self.items.iter().filter(|i| i.verified).count() as u32;
        Some(StagingDisposition {
            staging: self.staging?,
            items_total: self.items.len() as u32,
            items_verified,
            items_unverified: self.items.len() as u32 - items_verified,
            refusal_codes: self.refusal_codes.clone(),
            resumable_before_cancel: self.resumable_before_cancel,
            resume_possible: false,
        })
    }

    /// Items that hold a journaled identity, with their journaled names.
    pub fn held(&self) -> impl Iterator<Item = StagedItem> + '_ {
        self.items.iter().enumerate().filter_map(|(i, it)| {
            Some(StagedItem {
                item: i as u32,
                name: it.name.clone()?,
                id: it.id?,
                planned_ft: it.planned_ft,
            })
        })
    }

    /// A durable cause, whether recorded as a live fence or terminal metadata. This is attribution,
    /// not admission: terminal metadata never sets `fenced` or changes the epoch.
    pub fn has_fence_cause(&self, cause: FenceCause) -> bool {
        self.fence_causes.contains(&cause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_inside_staging() {
        for ok in ["a", "a/b", "dir/file.txt", "x y/z"] {
            assert!(relative_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "/a",
            "a/",
            "a//b",
            "..",
            "a/../b",
            ".",
            "C:",
            "C:/x",
            "a\\b",
            "\\\\srv\\x",
            "a:stream",
            "a\0b",
            "a\nb",
            "*",
        ] {
            assert!(!relative_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn entry_round_trips_through_json() {
        let e = Entry {
            epoch: 3,
            record: Record::CreateBatch {
                items: vec![(0, "a".into()), (1, "a/b".into())],
            },
        };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Entry>(&s).unwrap(), e);
        let w = Entry {
            epoch: 1,
            record: Record::PublishWithdrawn,
        };
        let s = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<Entry>(&s).unwrap(), w);
    }
}
