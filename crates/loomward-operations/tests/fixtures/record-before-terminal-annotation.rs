// Exact Record/Entry decoder from main 828ef592, before #738 terminal metadata.
// Retained only to test the rollback policy: unknown kinds fail closed.
use loomward_operations::{FenceCause, ObjectId, OpState, Refusal};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub epoch: u64,
    #[serde(flatten)]
    pub record: Record,
}
