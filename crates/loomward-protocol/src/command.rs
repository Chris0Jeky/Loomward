//! The closed list of commands (docs/41 section 5.2, `contracts/v3/commands.json`).
//!
//! The table below is the Rust copy of `commands.json`; the `commands_json` test fails if
//! the two ever differ. Adding a command is a coordinator-reviewed contract change.

use crate::dto::*;
use crate::envelope::{decode_exact, Payload};
use crate::error::ErrorBody;
use crate::event::EventName;
use crate::types::{Adapter, CommandName};
use serde::{Deserialize, Serialize};
use serde_json::Value;

macro_rules! commands {
    ($($var:ident = $wire:literal, $req:ident, $res:ident, [$($ad:ident),*], $wave:literal, [$($ev:ident),*];)*) => {
        /// A command the service understands. Every command has `effects: none`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum Command {
            $(#[serde(rename = $wire)] $var,)*
        }

        impl Command {
            pub const ALL: &'static [Command] = &[$(Command::$var),*];

            /// The wire name, e.g. `tree.slice`.
            pub const fn as_str(self) -> &'static str {
                match self { $(Command::$var => $wire,)* }
            }

            /// The `$defs` name the request payload is decoded as.
            pub const fn request_def(self) -> &'static str {
                match self { $(Command::$var => stringify!($req),)* }
            }

            /// Decodes `payload` as this command's request DTO, exactly (no unknown fields, in bounds).
            pub fn validate_request(self, payload: &Payload) -> Result<(), ErrorBody> {
                match self {
                    $(Command::$var => decode_exact::<$req>(Value::Object(payload.clone())).map(drop),)*
                }
            }

            /// The `$defs` name of the `result` of a successful response.
            pub const fn result_def(self) -> &'static str {
                match self { $(Command::$var => stringify!($res),)* }
            }

            /// Decodes `result` as this command's result DTO, exactly.
            pub fn validate_result(self, result: &Payload) -> Result<(), ErrorBody> {
                match self {
                    $(Command::$var => decode_exact::<$res>(Value::Object(result.clone())).map(drop),)*
                }
            }

            /// Adapters where the command exists; elsewhere it answers `capability_unavailable`.
            pub const fn adapters(self) -> &'static [Adapter] {
                match self { $(Command::$var => &[$(Adapter::$ad),*],)* }
            }

            /// The implementation wave in docs/43.
            pub const fn wave(self) -> u8 {
                match self { $(Command::$var => $wave,)* }
            }

            /// Events the command may cause.
            pub const fn events(self) -> &'static [EventName] {
                match self { $(Command::$var => &[$(EventName::$ev),*],)* }
            }
        }
    };
}

commands! {
    SessionHello = "session.hello", EmptyRequest, SessionInfo, [Tauri, Http], 1, [];
    HealthGet = "health.get", EmptyRequest, Health, [Tauri, Http], 2, [];
    RootsList = "roots.list", EmptyRequest, RootList, [Tauri, Http], 2, [];
    RootsRequestGrant = "roots.request_grant", RootGrantRequest, RootGrantResult, [Tauri], 2, [];
    RootsRevoke = "roots.revoke", RootRevokeRequest, RootRevokeResult, [Tauri, Http], 2, [];
    VolumesList = "volumes.list", EmptyRequest, VolumeList, [Tauri, Http], 2, [];
    VolumesDeclareTier = "volumes.declare_tier", DeclareTierRequest, DeclareTierResult, [Tauri, Http], 2, [];
    ScanStart = "scan.start", ScanStartRequest, JobResult, [Tauri, Http], 2, [JobState, ScanProgress, TreeInvalidated];
    ScanCancel = "scan.cancel", JobRefRequest, JobResult, [Tauri, Http], 2, [JobState];
    JobsGet = "jobs.get", JobRefRequest, JobResult, [Tauri, Http], 2, [];
    JobsList = "jobs.list", JobListRequest, JobList, [Tauri, Http], 2, [];
    TreeSlice = "tree.slice", TreeSliceRequest, TreeSlice, [Tauri, Http], 2, [];
    TreeChildren = "tree.children", TreeChildrenRequest, EntryPage, [Tauri, Http], 2, [];
    TreePath = "tree.path", NodeRefRequest, NodePath, [Tauri, Http], 2, [];
    NodeInspect = "node.inspect", NodeRefRequest, NodeDetail, [Tauri, Http], 2, [];
    ThreadsMeaning = "threads.meaning", MeaningRequest, MeaningResult, [Tauri, Http], 3, [];
    SearchQuery = "search.query", SearchRequest, EntryPage, [Tauri, Http], 2, [];
    StatsBreakdown = "stats.breakdown", BreakdownRequest, Breakdown, [Tauri, Http], 2, [];
    TaxonomyGet = "taxonomy.get", EmptyRequest, Taxonomy, [Tauri, Http], 3, [];
    FeedbackRecord = "feedback.record", FeedbackRequest, FeedbackResult, [Tauri, Http], 3, [LearningUpdated];
    LearningStatus = "learning.status", EmptyRequest, LearningStatus, [Tauri, Http], 3, [];
    LearningQueue = "learning.queue", LearningQueueRequest, LearningQueue, [Tauri, Http], 3, [];
    LearningRefit = "learning.refit", EmptyRequest, JobResult, [Tauri, Http], 3, [JobState, LearningUpdated];
    CollectionsList = "collections.list", EmptyRequest, CollectionList, [Tauri, Http], 3, [];
    CollectionsCreate = "collections.create", CollectionCreateRequest, Collection, [Tauri, Http], 3, [];
    CollectionsUpdateMembers = "collections.update_members", CollectionMembersUpdate, Collection, [Tauri, Http], 3, [];
    CollectionsMembers = "collections.members", CollectionMembersRequest, EntryPage, [Tauri, Http], 3, [];
    TeacherPreview = "teacher.preview", TeacherPreviewRequest, TeacherPreview, [Tauri, Http], 3, [];
    GrantsList = "grants.list", EmptyRequest, GrantList, [Tauri, Http], 2, [];
    GrantsCreateDisclosure = "grants.create_disclosure", DisclosureGrantRequest, DisclosureGrantResult, [Tauri, Http], 3, [];
    GrantsRevoke = "grants.revoke", GrantRevokeRequest, GrantRevokeResult, [Tauri, Http], 2, [];
    TeacherRun = "teacher.run", TeacherRunRequest, JobResult, [Tauri, Http], 3, [JobState, LearningUpdated];
    TeacherResults = "teacher.results", JobRefRequest, TeacherResults, [Tauri, Http], 3, [];
    TiersModel = "tiers.model", EmptyRequest, TierModel, [Tauri, Http], 2, [];
    PlacementCandidates = "placement.candidates", PlacementCandidatesRequest, PlacementCandidates, [Tauri, Http], 2, [];
    PlacementSimulate = "placement.simulate", PlacementSimulateRequest, PlacementPlan, [Tauri, Http], 2, [];
    ProposalsList = "proposals.list", EmptyRequest, ProposalList, [Tauri, Http], 2, [];
    ProposalsGet = "proposals.get", ProposalRefRequest, ProposalDetail, [Tauri, Http], 2, [];
    TelemetrySubscribe = "telemetry.subscribe", TelemetrySubscribeRequest, TelemetrySubscription, [Tauri, Http], 2, [TelemetrySample];
    TelemetryUnsubscribe = "telemetry.unsubscribe", SubscriptionRefRequest, TelemetryUnsubscribed, [Tauri, Http], 2, [];
    TelemetrySnapshot = "telemetry.snapshot", TelemetrySnapshotRequest, TelemetrySample, [Tauri, Http], 2, [];
    ProcessesList = "processes.list", ProcessListRequest, ProcessList, [Tauri, Http], 2, [];
    ProcessesExplain = "processes.explain", ProcessRefRequest, ProcessExplanation, [Tauri, Http], 2, [];
    BudgetsGet = "budgets.get", EmptyRequest, OwnBudgets, [Tauri, Http], 2, [];
    BudgetsSet = "budgets.set", BudgetSetRequest, OwnBudgets, [Tauri, Http], 2, [];
}

impl Command {
    /// `None` for a name outside the closed list: the caller answers `unknown_command`.
    pub fn from_name(name: &str) -> Option<Command> {
        Command::ALL.iter().copied().find(|c| c.as_str() == name)
    }

    pub fn available_on(self, adapter: Adapter) -> bool {
        self.adapters().contains(&adapter)
    }

    pub fn name(self) -> CommandName {
        CommandName::new(self.as_str())
            .expect("every Command wire name matches the CommandName pattern")
    }
}

/// Substrings no command name may contain: a name that reads like an effect is a contract
/// violation (invariant 1), whatever it claims to do.
pub const FORBIDDEN_NAME_PARTS: &[&str] = &[
    "delete",
    "move",
    "rename",
    "kill",
    "exec",
    "shell",
    "trim",
    "suspend",
    "priority",
    "uninstall",
    "write_file",
    "run_command",
];
