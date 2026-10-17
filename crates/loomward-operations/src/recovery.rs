//! Recovery reader (doc 47 section 5, "Crash between" table). From the journal plus an injected observation
//! of the destination, decide per operation what an owner Resume or Abandon would do. Nothing here acts: a
//! restart never resumes an effect by itself, and the executor re-checks every identity before any removal.
use crate::journal::{Invalid, ObjectId, Replay, Row, StagedItem};
use crate::state::OpState;

/// What the destination shows. Implementations open every name relative to a pinned handle (lane C2);
/// tests fake it.
pub trait Disk {
    /// Identity under a single-component `name` directly in the pinned destination parent.
    fn in_parent(&self, name: &str) -> Option<ObjectId>;
    /// Entry under the staging-relative `name`, opened through the staging folder whose identity is `staging`.
    fn in_staging(&self, staging: ObjectId, name: &str) -> Option<Observed>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    pub id: ObjectId,
    pub links: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attention {
    /// The journal does not replay: truncated, corrupt, duplicated, out of order or illegal. Nothing is guessed.
    InvalidJournal(Invalid),
    /// The journal itself records `needs_attention`.
    Recorded,
    /// The staging name no longer resolves to the journaled staging identity (and it was not published).
    StagingMismatch,
    /// Entries under journaled names whose create was never identity-committed: never adopted or deleted.
    Unjournaled(Vec<String>),
    /// A journaled name resolves to another identity, a link count other than 1, or a verified item vanished.
    EntryMismatch(Vec<u32>),
}

/// What an owner Resume continues with.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resume {
    /// The staging folder was intended but its identity never committed: make a new one with a new token.
    pub new_staging: bool,
    /// Journaled, unverified and still under their names: re-verify by ID; on mismatch remove by identity, redo.
    pub reverify: Vec<StagedItem>,
    /// Journaled but gone from disk (removed before its record): journal the removal, then redo.
    pub gone: Vec<StagedItem>,
    /// A publish intent is pending and staging is intact: re-read the epoch, then rename again or withdraw.
    pub publish: bool,
}

/// Removal of Loomward's own journaled staging only. Every `remove` entry was observed under its journaled
/// name with its journaled identity and one link; the executor re-checks that before each removal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cleanup {
    pub staging: Option<ObjectId>,
    /// Journal `publish_withdrawn` first (fenced while the publish intent was pending, rename not done).
    pub withdraw_publish: bool,
    pub remove: Vec<StagedItem>,
    pub gone: Vec<StagedItem>,
    pub remove_staging: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Terminal: nothing to do.
    Settled,
    /// Approval consumed, nothing created: approved again only with a fresh confirmation, else cancelled.
    Reconfirm,
    Resume(Resume),
    Cleanup(Cleanup),
    /// The final name carries the staging identity: reacquire holds, run the post-publish confirmation with a
    /// full re-hash, then `committed` or `needs_attention`. Never cleanup.
    Confirm {
        staging: ObjectId,
        final_name: String,
    },
    NeedsAttention(Attention),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    pub op: u64,
    /// `None` when the journal does not replay.
    pub state: Option<OpState>,
    pub decision: Decision,
    /// Staging tokens whose identity was never committed: possible empty leftovers for the owner to remove.
    pub leftover_staging: Vec<String>,
}

pub fn recover(op: u64, rows: &[Row], disk: &impl Disk) -> Recovery {
    match Replay::from_rows(op, rows) {
        Err(e) => Recovery {
            op,
            state: None,
            decision: Decision::NeedsAttention(Attention::InvalidJournal(e)),
            leftover_staging: Vec::new(),
        },
        Ok(r) => {
            let leftover_staging = match r.staging {
                Some(_) => r.staging_tokens[..r.staging_tokens.len() - 1].to_vec(),
                None => r.staging_tokens.clone(),
            };
            Recovery {
                op,
                state: Some(r.state),
                decision: decide(&r, disk),
                leftover_staging,
            }
        }
    }
}

fn decide(r: &Replay, disk: &impl Disk) -> Decision {
    use OpState::*;
    let attention = |a| Decision::NeedsAttention(a);
    if r.state.is_terminal() {
        return Decision::Settled;
    }
    // A pending publish whose final name now carries the staging identity was renamed: confirm, never clean.
    if let (Some(s), Some(final_name)) = (r.staging, &r.publish) {
        if disk
            .in_parent(final_name)
            .is_some_and(|o| s.matches(&o, r.staging_planned))
        {
            return if r.state == Publishing {
                Decision::Confirm {
                    staging: s,
                    final_name: final_name.clone(),
                }
            } else {
                attention(Attention::Recorded)
            };
        }
    }
    if r.state == NeedsAttention && !r.fenced {
        return attention(Attention::Recorded);
    }
    let cancel = r.fenced || r.state == Cancelling;
    let Some(staging) = r.staging else {
        // Nothing journaled was created (doc 47 section 5 rows 1 and 2).
        return if cancel {
            Decision::Cleanup(Cleanup::default())
        } else if r.staging_tokens.is_empty() {
            Decision::Reconfirm
        } else {
            Decision::Resume(Resume {
                new_staging: true,
                ..Resume::default()
            })
        };
    };
    if r.staging_removed || r.staging_left {
        return Decision::Cleanup(Cleanup {
            staging: Some(staging),
            ..Cleanup::default()
        });
    }
    let token = r
        .staging_tokens
        .last()
        .expect("staging identity has a token");
    if !disk
        .in_parent(token)
        .is_some_and(|o| staging.matches(&o, r.staging_planned))
    {
        return attention(Attention::StagingMismatch);
    }
    // Names never identity-committed must be absent: never adopted, never deleted (row 3).
    let unjournaled: Vec<String> = r
        .items
        .iter()
        .filter(|i| i.id.is_none())
        .filter_map(|i| i.name.clone())
        .filter(|n| disk.in_staging(staging, n).is_some())
        .collect();
    if !unjournaled.is_empty() {
        return attention(Attention::Unjournaled(unjournaled));
    }
    let (mut present, mut gone, mut mismatched) = (Vec::new(), Vec::new(), Vec::new());
    for held in r.held() {
        let verified = r.items[held.item as usize].verified;
        match disk.in_staging(staging, &held.name) {
            Some(o) if held.id.matches(&o.id, held.planned_ft) && o.links == 1 => {
                present.push(held)
            }
            None if !verified || cancel => gone.push(held),
            _ => mismatched.push(held.item),
        }
    }
    if !mismatched.is_empty() {
        return attention(Attention::EntryMismatch(mismatched));
    }
    if cancel {
        return Decision::Cleanup(Cleanup {
            staging: Some(staging),
            withdraw_publish: r.publish.is_some(),
            remove: present,
            gone,
            remove_staging: true,
        });
    }
    let reverify = present
        .into_iter()
        .filter(|h| !r.items[h.item as usize].verified)
        .collect();
    Decision::Resume(Resume {
        new_staging: false,
        reverify,
        gone,
        publish: r.state == Publishing,
    })
}
