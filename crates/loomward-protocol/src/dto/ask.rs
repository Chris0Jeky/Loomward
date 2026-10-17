//! Ask (LW-118, docs/46 section 8): the owner's provider profile, the `ask:local` client grant and
//! one model turn over the read-only tool catalogue. Mirrors `contracts/v3/view-service.schema.json`;
//! the contract tests keep them equal.
//!
//! The model has no effect and no approval authority: every tool is a read, an answer is text, and
//! citations are resolved from the turn's ledger of handles, never from the answer's wording.

use super::*;

const_str!(
    pub AskPrincipal = "ask:local"
);

/// The optional fields an Ask grant may disclose to the model (LW-067 `CLIENT_FIELDS`). Name,
/// kind, extension, coverage and match type are always part of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskField {
    Path,
    Size,
    Modified,
    Attributes,
}

/// `local` is the only class that runs: a cloud profile is refused when configured (q-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskProviderState {
    NotConfigured,
    Local,
}

/// An owner-entered loopback endpoint and model. There is no default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskProfile {
    pub base_url: BoundedStr<1, 160>,
    pub model: BoundedStr<1, 128>,
}

/// Tauri adapter only (docs/46 section 8.1: profiles are created on the desktop). `null` clears it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskConfigureRequest {
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub profile: Option<AskProfile>,
}

/// Replaces the `ask:local` grant: a new epoch, so an old cursor dies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskGrantRequest {
    pub roots: UniqueVec<RootId, 1, 64>,
    pub fields: UniqueVec<AskField, 0, 4>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskGrant {
    /// Granted roots whose own root grant is still active.
    pub roots: BoundedVec<RootId, 0, 64>,
    pub fields: BoundedVec<AskField, 0, 4>,
    pub granted_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskTool {
    pub name: Text<32>,
    pub backed_by: CommandName,
    pub read_only: ConstTrue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskStatus {
    pub provider: AskProviderState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub profile: Option<AskProfile>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub profile_digest: Option<Digest>,
    pub principal: AskPrincipal,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub grant: Option<AskGrant>,
    pub tools: BoundedVec<AskTool, 1, 16>,
    /// The latest turn of this session, if any (turns are held in memory only).
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub last_turn: Option<AskTurnId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskTurnRequest {
    pub question: BoundedStr<1, 2000>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskTurnRef {
    pub turn_id: AskTurnId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskTurnState {
    Running,
    Completed,
    Failed,
}

/// Why a turn ended without an answer (the provider's error kinds, docs/46 section 8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskFailureKind {
    ConsentRequired,
    CloudUnavailable,
    NotLocal,
    InvalidRequest,
    Unreachable,
    Timeout,
    HttpStatus,
    Oversize,
    InvalidReply,
    BrokerFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskFailure {
    pub kind: AskFailureKind,
    pub message: Text<256>,
    /// False only when the turn provably sent nothing.
    pub dispatched: bool,
}

/// A handle the answer cites, resolved through the turn's ledger. `resolved` is false when the
/// entry no longer reads (revoked, forgotten or rescanned away): the chip says so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskCitation {
    pub handle: Text<8>,
    pub node_id: NodeId,
    pub resolved: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub name: Option<Text<255>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub root_id: Option<RootId>,
}

/// One tool call as the model made it: never its arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskToolUse {
    pub name: Text<64>,
    pub ok: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub rows: Option<Count>,
    pub detail: Text<128>,
}

/// One model round's disclosure record: what was sent, by digest and length, and to which profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskRound {
    pub round: Int<1, 9>,
    pub profile_digest: Digest,
    pub principal: AskPrincipal,
    pub sha256: Digest,
    pub bytes: Count,
    pub dispatched: bool,
    /// `pending`, `ok` or an `AskFailureKind`.
    pub outcome: Text<32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskTurnRecord {
    pub turn_id: AskTurnId,
    pub state: AskTurnState,
    pub question: BoundedStr<1, 2000>,
    pub started_at: Timestamp,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub finished_at: Option<Timestamp>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub answer: Option<Text<16000>>,
    pub answer_truncated: bool,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub failure: Option<AskFailure>,
    /// Every ledger-backed handle the answer cites (a turn mints at most 160).
    pub citations: BoundedVec<AskCitation, 0, 160>,
    /// Handles the answer names that the ledger does not hold: shown as unverified references.
    pub unverified: BoundedVec<Text<8>, 0, 64>,
    pub tool_calls: BoundedVec<AskToolUse, 0, 64>,
    pub tool_calls_total: Count,
    pub rounds: BoundedVec<AskRound, 0, 9>,
}
