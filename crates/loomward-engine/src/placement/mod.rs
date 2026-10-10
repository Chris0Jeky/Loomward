//! Placement: tier model, ancestry-antichain candidates and bounded simulation (docs/41 section 10).
//!
//! Owner: lane L12 (LW-108, LW-030 wiring). The budgeted link-count pass is
//! `loomward-windows::linkcount`; the planner is `loomward-core` (v1, and the v2 port of #107).
//!
//! Scope: three byte quantities (entry, allocated, relief), `relief_basis` and `estimate_basis`,
//! cluster-rounded destination estimate, `pre_rejected`, `excluded_volumes`; unknown flags and
//! heat are omitted from the planner scenario, never `null`. Simulation only: nothing moves.
//!
//! #117 errata items this lane applies, each with its test before merge:
//! - Partial #4 relief mapping: planner `source_bytes` = `estimated_relief_bytes` under the
//!   verified policy. Verified relief requires known allocation and identity-matched link
//!   observations; passing entry bytes directly reproduces the "allocations exceed used
//!   capacity" rejection.
//! - N5 slow placement: `placement.simulate` stays synchronous and bounded by `node_budget`,
//!   returning `deadline_exceeded` (or `resource_budget`). There is no job variant in v0.3.
use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::*;
use loomward_windows::identity::{IdentityQuality, ObjectKey, ObservedIdentity};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

/// Observed volume physics and the owner's reserve preference (never request overrides).
#[derive(Debug, Clone)]
pub struct VolumeObservation {
    /// Wire observation, including declaration and device hints.
    pub volume: Volume,
    /// Minimum remaining free bytes.
    pub reserve_bytes: Bytes,
    /// Allocation unit, unknown when the native query failed.
    pub cluster_bytes: Option<u64>,
    /// Native serial binding file identities to this volume; unknown prevents verified relief.
    pub volume_serial: Option<String>,
}

/// Catalogue directory metadata before placement estimates are derived.
#[derive(Debug, Clone)]
pub struct Subtree {
    /// Catalogue-issued group reference.
    pub group_id: GroupId,
    /// Catalogue-issued node reference.
    pub node_id: NodeId,
    /// Granted root reference.
    pub root_id: RootId,
    /// Display name only.
    pub name: Text<260>,
    /// Complete root-to-parent node chain, including across overlapping granted roots.
    pub ancestors: Vec<NodeId>,
    /// Whether this node is a direct child of a granted root.
    pub root_child: bool,
    /// Revision-consistent entry totals, default stream only.
    pub totals: SubtreeTotals,
    /// Last modification, never access heat.
    pub newest_modified_at: Option<Timestamp>,
    /// Age at the snapshot time, for explicitly requested modification-age what-ifs only.
    pub modified_age_days: Option<u64>,
    /// Known pin preference; unknown remains unknown.
    pub pinned: Option<bool>,
    /// Known active state.
    pub active: Option<bool>,
    /// Known protection state.
    pub protected: Option<bool>,
    /// Known movement history.
    pub days_since_move: Option<Int<0, 36500>>,
}

/// One listed name and its point-in-time link observation.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Unique catalogue row reference; repeated rows never count as distinct names.
    pub node_id: NodeId,
    /// Default-stream EOF.
    pub logical_bytes: u64,
    /// Listed default-stream allocation, unknown on an unsupported listing.
    pub allocated_bytes: Option<u64>,
    /// Listed native identity, not an effect grant.
    pub object: Option<ObjectKey>,
    /// Metadata-only observation from the budgeted link-count pass.
    pub link_observation: Option<ObservedIdentity>,
}

/// A consistent, bounded placement read. Subtrees all belong to the requested volume.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// At most 64 volumes; unknown physics is retained in the tier view.
    pub volumes: Vec<VolumeObservation>,
    /// At most 10,000 directory headers, before antichain selection.
    pub subtrees: Vec<Subtree>,
    /// Generations binding this read and its subsequent file observations.
    pub root_generations: Vec<RootGeneration>,
}

/// Narrow L2 integration boundary. Implementations must honour deadlines and generation bindings.
pub trait PlacementInput {
    /// Read volumes and, when present, directory headers on the source volume.
    fn snapshot(&self, source: Option<&VolumeId>, deadline: Instant) -> EngineResult<Snapshot>;
    /// Read at most 200,000 distinct names and identity-matched link observations for this subtree.
    /// Missing observations are returned as unknown; stale generations return `StaleGeneration`.
    fn files(
        &self,
        subtree: &Subtree,
        generations: &[RootGeneration],
        deadline: Instant,
    ) -> EngineResult<Vec<FileEntry>>;
}

/// L8's durable state boundary. Save is atomic and checks the deadline before its commit.
pub trait ProposalStore {
    /// Save the complete plan or nothing; assign its ID and actual creation time in the store.
    fn save(
        &self,
        source: &VolumeId,
        digest: &Digest,
        plan: &PlacementPlan,
        deadline: Instant,
    ) -> EngineResult<PlacementPlan>;
    /// List up to 100 visible proposals, marking changed generations stale.
    fn list(&self) -> EngineResult<ProposalList>;
    /// Read a proposal and its input digest; unknown IDs return `NotFound`.
    fn get(&self, request: &ProposalRefRequest) -> EngineResult<ProposalDetail>;
}

/// The exact reference-planner input plus separately labelled service exclusions.
#[derive(Debug)]
pub struct BuiltScenario {
    /// Numbers use the reference's safe-integer range; unknown optional fields are absent.
    pub scenario: Value,
    /// Candidate observations, including original unknowns.
    pub candidates: PlacementCandidates,
    /// Groups excluded before planner validation.
    pub pre_rejected: Vec<PlacementPlanPreRejected>,
    /// Volumes excluded before planner validation.
    pub excluded_volumes: Vec<PlacementPlanExcludedVolume>,
    /// Explicit what-if and destination-estimate limitations.
    pub assumptions: Vec<Text<256>>,
}

impl Engine {
    /// Tier view including unknown and offline volumes, derived from trusted observations.
    pub fn placement_model(
        &self,
        input: &dyn PlacementInput,
        deadline: Instant,
    ) -> EngineResult<TierModel> {
        check_deadline(deadline)?;
        let snapshot = input.snapshot(None, deadline)?;
        if snapshot.volumes.len() > 64
            || snapshot.subtrees.len() > 10_000
            || snapshot.root_generations.len() > 64
        {
            return Err(EngineError::ResourceBudget {
                message: "placement snapshot bounds".into(),
            });
        }
        {
            let mut ids = BTreeSet::new();
            for observed in &snapshot.volumes {
                if !ids.insert(observed.volume.volume_id.as_str()) {
                    return Err(invalid("duplicate volume"));
                }
            }
        }
        let mut volumes = Vec::new();
        for observed in snapshot.volumes {
            check_deadline(deadline)?;
            let VolumeObservation {
                volume: v,
                reserve_bytes,
                cluster_bytes,
                ..
            } = observed;
            // #159-5: unusable numbers stay visible as Unknown instead of
            // failing the whole tier view.
            let unusable = v.online
                && (matches!(
                    (v.capacity_bytes, v.free_bytes),
                    (Some(cap), Some(free)) if free.get() > cap.get()
                ) || matches!(v.capacity_bytes, Some(cap) if reserve_bytes.get() > cap.get())
                    || matches!(v.capacity_bytes, Some(cap) if cap.get() == 0)
                    || cluster_bytes == Some(0));
            let fraction = if unusable {
                None
            } else {
                match (v.capacity_bytes, v.free_bytes) {
                    (Some(capacity), Some(free))
                        if v.online && capacity.get() > 0 && free <= capacity =>
                    {
                        Some(bounded(Fraction::new(
                            free.get() as f64 / capacity.get() as f64,
                        ))?)
                    }
                    _ => None,
                }
            };
            let pressure = match fraction {
                Some(f) if f.get() <= 0.05 => TierVolumePressure::Pressure,
                Some(f) if f.get() <= 0.15 => TierVolumePressure::Watch,
                Some(_) => TierVolumePressure::Ok,
                None => TierVolumePressure::Unknown,
            };
            let tier = tier_info(&v);
            volumes.push(TierVolume {
                volume_id: v.volume_id,
                display_name: v.display_name,
                tier,
                online: v.online,
                writable: v.read_only.map(|ro| !ro),
                capacity_bytes: v.capacity_bytes,
                free_bytes: v.free_bytes,
                reserve_bytes,
                free_fraction: fraction,
                pressure,
            });
        }
        Ok(TierModel {volumes: bounded(BoundedVec::new(volumes))?, policy: TierModelPolicy {
            watch_free_fraction: Fraction::new(0.15).expect("fraction"), pressure_free_fraction: Fraction::new(0.05).expect("fraction"),
            reserve_note: Text::new("Reserves are observed owner preferences, not request overrides.").expect("bounded note"),
            note: Text::new("Tier 0 is fastest; device hints are not benchmarks. Unknown and offline volumes remain visible.").expect("bounded note")}})
    }
    /// Antichain candidates with entry quantities and separately verified relief.
    pub fn placement_candidates(
        &self,
        input: &dyn PlacementInput,
        request: &PlacementCandidatesRequest,
        deadline: Instant,
    ) -> EngineResult<PlacementCandidates> {
        check_deadline(deadline)?;
        let snapshot = input.snapshot(Some(&request.source_volume_id), deadline)?;
        let _ = planner_volumes(&snapshot)?;
        require_source(&snapshot, &request.source_volume_id)?;
        let (cluster, _) = target_cluster(&snapshot, &request.source_volume_id);
        Ok(candidates(
            input,
            &snapshot,
            &request.source_volume_id,
            request.basis,
            request.max_groups.get() as usize,
            safe(request.min_bytes.get())?,
            cluster,
            deadline,
        )?
        .view)
    }
    /// Synchronous node- and time-bounded simulation. Save requires an atomic proposal store.
    pub fn placement_simulate(
        &self,
        input: &dyn PlacementInput,
        request: &PlacementSimulateRequest,
        deadline: Instant,
        store: Option<&dyn ProposalStore>,
    ) -> EngineResult<PlacementPlan> {
        check_deadline(deadline)?;
        if request.save && store.is_none() {
            return Err(EngineError::unavailable(Component::Placement));
        }
        let built = build_scenario(input, request, deadline)?;
        let digest = if request.save {
            Some(
                Digest::new(
                    format!("sha256:{:x}", Sha256::digest(serde_json::to_vec(&json!({
                "scenario": built.scenario, "request": request, "candidates": built.candidates,
                "pre_rejected": built.pre_rejected, "excluded_volumes": built.excluded_volumes,
                "assumptions": built.assumptions
            })).expect("serializable input"))),
                )
                .expect("SHA256 digest"),
            )
        } else {
            None
        };
        let scenario = serde_json::from_value(built.scenario)
            .map_err(|_| invalid("invalid planner scenario"))?;
        check_deadline(deadline)?;
        let result =
            loomward_core::planner_v2::plan_v2(&scenario, request.node_budget.get() as u64)
                .map_err(|message| EngineError::InvalidRequest { message })?;
        check_deadline(deadline)?;
        let mut value = serde_json::to_value(result).expect("serializable core plan");
        for field in [
            "target_free_bytes",
            "shortfall_bytes",
            "transfer_bytes",
            "baseline_shortfall_bytes",
            "shortfall_improvement_bytes",
        ] {
            value[field] = json!(value[field].to_string());
        }
        let lower_bound = &mut value["search"]["lower_bound_shortfall_bytes"];
        *lower_bound = json!(lower_bound.to_string());
        for proposal in value["proposals"].as_array_mut().expect("proposal array") {
            for field in [
                "source_bytes_relieved",
                "destination_bytes_required",
                "transfer_bytes",
            ] {
                proposal[field] = json!(proposal[field].to_string());
            }
        }
        for free in value["projected_free_bytes"]
            .as_object_mut()
            .expect("free map")
            .values_mut()
        {
            *free = json!(free.to_string());
        }
        value["pre_rejected"] = json!(built.pre_rejected);
        value["excluded_volumes"] = json!(built.excluded_volumes);
        value["relief_policy"] = json!(request.relief_policy);
        value["heat_policy"] = json!(request.heat_policy);
        value["assumptions"] = json!(built.assumptions);
        value["root_generations"] = json!(built.candidates.root_generations);
        value["proposal_id"] = Value::Null;
        let plan: PlacementPlan =
            serde_json::from_value(value).map_err(|_| EngineError::Internal {
                message: "core plan violates placement wire contract".into(),
                detail: None,
            })?;
        check_deadline(deadline)?;
        if request.save {
            let saved = store.expect("store required above").save(
                &request.source_volume_id,
                &digest.expect("save digest"),
                &plan,
                deadline,
            )?;
            // Once the store has committed, report the saved proposal even past the deadline:
            // a committed mutation must not read as a failure that invites a duplicate retry.
            Ok(saved)
        } else {
            Ok(plan)
        }
    }
    /// Saved simulations; the store controls visibility and staleness.
    pub fn proposals_list(&self, store: &dyn ProposalStore) -> EngineResult<ProposalList> {
        store.list()
    }
    /// A stored simulation and the digest of its actual inputs, never an executable manifest.
    pub fn proposals_get(
        &self,
        store: &dyn ProposalStore,
        request: &ProposalRefRequest,
    ) -> EngineResult<ProposalDetail> {
        store.get(request)
    }
}

/// Build the bounded antichain scenario; never trusts physical quantities from the request.
pub fn build_scenario(
    input: &dyn PlacementInput,
    request: &PlacementSimulateRequest,
    deadline: Instant,
) -> EngineResult<BuiltScenario> {
    check_deadline(deadline)?;
    let snapshot = input.snapshot(Some(&request.source_volume_id), deadline)?;
    check_deadline(deadline)?;
    let (volumes, excluded_volumes) = planner_volumes(&snapshot)?;
    require_source(&snapshot, &request.source_volume_id)?;
    if volumes.len() > 32 {
        return Err(EngineError::ResourceBudget {
            message: "reference planner supports 32 volumes".into(),
        });
    }
    let (cluster, unknown_clusters) = target_cluster(&snapshot, &request.source_volume_id);
    let candidates = candidates(
        input,
        &snapshot,
        &request.source_volume_id,
        request.candidate_basis,
        request.max_groups.get() as usize,
        0,
        cluster,
        deadline,
    )?;
    let mut seen_overrides = BTreeSet::new();
    for o in request.overrides.iter() {
        if !seen_overrides.insert(o.group_id.as_str())
            || !candidates
                .view
                .groups
                .iter()
                .any(|g| g.group_id == o.group_id)
        {
            return Err(invalid("duplicate_or_unknown_group_override"));
        }
    }
    let mut assumptions = vec![Text::new("Destination estimates round each default-stream EOF to the largest known eligible target cluster; sparse, compressed and alternate streams are not predicted.").expect("bounded note")];
    if unknown_clusters > 0 {
        assumptions.push(
            Text::new(format!(
                "Destination estimates assumed 4 KiB clusters for {unknown_clusters} target(s) with unknown cluster size."
            ))
            .expect("bounded note"),
        );
    }
    if request.relief_policy == PlacementSimulateRequestReliefPolicy::EntryAllocationWhatif {
        assumptions.push(Text::new("Entry allocation is assumed reclaimable for this what-if; hard links outside a group may prevent relief.").expect("bounded note"));
    }
    if request.heat_policy == PlacementSimulateRequestHeatPolicy::MtimeProxyWhatif {
        assumptions.push(Text::new("Modification age is not access heat: >=30 days maps to 0.1, >=7 to 0.5, otherwise 1.0; missing age stays unknown.").expect("bounded note"));
    }
    if !request.overrides.is_empty() {
        assumptions.push(Text::new("Owner heat, flag and movement-history overrides are assumptions, never operation grants.").expect("bounded note"));
    }
    let mut groups = Vec::new();
    let mut pre_rejected = Vec::new();
    let mut zero_excluded = 0usize;
    let shared = shared_groups(&candidates);
    for candidate in candidates.view.groups.iter() {
        check_deadline(deadline)?;
        let mut g = candidate.clone();
        let reason = if g.coverage != CoverageState::Complete {
            Some(PlacementPlanPreRejectedReason::GroupCoverageIncomplete)
        } else if shared.contains(g.group_id.as_str()) {
            Some(PlacementPlanPreRejectedReason::SharesObjectsWithOtherGroup)
        } else if g.estimated_relief_bytes.is_none()
            && (request.relief_policy == PlacementSimulateRequestReliefPolicy::VerifiedOnly
                || g.estimate_basis != CandidateGroupEstimateBasis::AllocatedEntries)
        {
            Some(PlacementPlanPreRejectedReason::ReliefUnknown)
        } else {
            None
        };
        if let Some(reason) = reason {
            pre_rejected.push(PlacementPlanPreRejected {
                group_id: g.group_id.clone(),
                reason,
            });
            continue;
        }
        if request.heat_policy == PlacementSimulateRequestHeatPolicy::MtimeProxyWhatif {
            let subtree = snapshot
                .subtrees
                .iter()
                .find(|s| s.group_id == g.group_id)
                .expect("selected subtree");
            g.heat = subtree.modified_age_days.map(|days| {
                Fraction::new(if days >= 30 {
                    0.1
                } else if days >= 7 {
                    0.5
                } else {
                    1.0
                })
                .expect("fraction")
            });
        }
        if let Some(o) = request.overrides.iter().find(|o| o.group_id == g.group_id) {
            if let Some(v) = o.heat {
                g.heat = v;
            }
            if let Some(v) = o.pinned {
                g.pinned = v;
            }
            if let Some(v) = o.active {
                g.active = v;
            }
            if let Some(v) = o.protected {
                g.protected = v;
            }
            if let Some(v) = o.days_since_move {
                g.days_since_move = v;
            }
        }
        let relief = if request.relief_policy
            == PlacementSimulateRequestReliefPolicy::EntryAllocationWhatif
        {
            g.source_bytes
        } else {
            g.estimated_relief_bytes.expect("verified above")
        };
        // #159-1: a verified zero relief would make the core planner reject
        // the whole request ("invalid or duplicate group"). No existing
        // pre-rejection reason honestly fits known zero relief, so exclude
        // the group and disclose the count; the rest still plans.
        if relief.get() == 0 {
            zero_excluded += 1;
            continue;
        }
        let mut group = json!({"id": g.group_id, "volume_id": request.source_volume_id,
            "source_bytes": safe(relief.get())?, "destination_bytes": safe(g.destination_bytes.get())?,
            "transfer_bytes": safe(g.transfer_bytes.get())?});
        if let Some(v) = g.heat {
            group["heat"] = json!(v.get());
        }
        if let Some(v) = g.pinned {
            group["pinned"] = json!(v);
        }
        if let Some(v) = g.active {
            group["active"] = json!(v);
        }
        if let Some(v) = g.protected {
            group["protected"] = json!(v);
        }
        if let Some(v) = g.days_since_move {
            group["days_since_move"] = json!(v.get());
        }
        groups.push(group);
    }
    if zero_excluded > 0 {
        assumptions.push(
            Text::new(format!(
                "Excluded {zero_excluded} group(s) with zero relief; the remaining groups were still planned."
            ))
            .expect("bounded note"),
        );
    }
    // #159-3: the greedy antichain hides children of a selected parent. When
    // that parent is later pre-rejected, its descendants are in neither
    // `groups` nor `pre_rejected`, so disclose the count. They are not re-planned.
    let pre_rejected_nodes: BTreeSet<&str> = pre_rejected
        .iter()
        .filter_map(|pr| {
            candidates
                .view
                .groups
                .iter()
                .find(|g| g.group_id == pr.group_id)
                .map(|g| g.node_id.as_str())
        })
        .collect();
    if !pre_rejected_nodes.is_empty() {
        let selected_ids: BTreeSet<&str> = candidates
            .view
            .groups
            .iter()
            .map(|g| g.group_id.as_str())
            .collect();
        let mut hidden_descendants = 0usize;
        for s in &snapshot.subtrees {
            if request.candidate_basis == CandidateBasis::RootChildren && !s.root_child {
                continue;
            }
            if s.totals.logical_bytes.get() == 0 {
                continue;
            }
            if selected_ids.contains(s.group_id.as_str()) {
                continue;
            }
            if s
                .ancestors
                .iter()
                .any(|a| pre_rejected_nodes.contains(a.as_str()))
            {
                hidden_descendants += 1;
            }
        }
        if hidden_descendants > 0 {
            assumptions.push(
                Text::new(format!(
                    "{hidden_descendants} descendant group(s) not considered because their ancestor was pre-rejected."
                ))
                .expect("bounded note"),
            );
        }
    }
    check_deadline(deadline)?;
    Ok(BuiltScenario {
        scenario: json!({"source_id": request.source_volume_id,
        "target_free_bytes": safe(request.target_free_bytes.get())?,
        "max_transfer_bytes": safe(request.max_transfer_bytes.get())?, "cooldown_days": 7,
        "volumes": volumes, "groups": groups}),
        candidates: candidates.view,
        pre_rejected,
        excluded_volumes,
        assumptions,
    })
}

fn invalid(message: &str) -> EngineError {
    EngineError::InvalidRequest {
        message: message.into(),
    }
}

fn check_deadline(deadline: Instant) -> EngineResult<()> {
    if Instant::now() >= deadline {
        Err(EngineError::DeadlineExceeded {
            message: "placement deadline exceeded".into(),
        })
    } else {
        Ok(())
    }
}

fn bounded<T>(result: Result<T, Invalid>) -> EngineResult<T> {
    result.map_err(|_| invalid("placement contract bounds exceeded"))
}

fn safe(n: u64) -> EngineResult<u64> {
    if n > MAX_SAFE_INTEGER {
        Err(invalid("planner bytes exceed 2^53-1"))
    } else {
        Ok(n)
    }
}

fn tier_info(v: &Volume) -> TierInfo {
    let hint = if v.device.basis == DeviceHintBasis::Unavailable {
        None
    } else {
        match (v.device.bus_type, v.device.seek_penalty) {
            (DeviceHintBusType::Usb, Some(true)) => Some(7),
            (_, Some(true)) => Some(5),
            (DeviceHintBusType::Nvme, Some(false)) => Some(1),
            (DeviceHintBusType::Sata, Some(false)) => Some(2),
            _ => None,
        }
    }
    .map(|t| Tier::new(t).expect("hint tier"));
    let declared = v.tier.declared_tier;
    TierInfo {
        tier: declared.or(hint),
        basis: if declared.is_some() {
            TierInfoBasis::Declared
        } else if hint.is_some() {
            TierInfoBasis::DeviceHint
        } else {
            TierInfoBasis::Unknown
        },
        declared_tier: declared,
        hint_tier: hint,
        note: Text::new("Declared tier, else device hint; hints are not measured speed.")
            .expect("bounded note"),
    }
}

/// Largest eligible target cluster, keeping targets whose cluster size is
/// unknown: they assume a stated 4 KiB lower bound (#159-2). Returns the
/// cluster and the count of eligible targets with unknown cluster size.
fn target_cluster(snapshot: &Snapshot, source: &VolumeId) -> (Option<u64>, usize) {
    let Some(source_tier) = snapshot
        .volumes
        .iter()
        .find(|v| v.volume.volume_id == *source)
        .and_then(|v| tier_info(&v.volume).tier)
    else {
        return (None, 0);
    };
    let eligible: Vec<_> = snapshot
        .volumes
        .iter()
        .filter(|v| {
            v.volume.volume_id != *source
                && v.volume.online
                && v.volume.read_only == Some(false)
                && v.volume.capacity_bytes.is_some()
                && v.volume.free_bytes.is_some()
                && tier_info(&v.volume)
                    .tier
                    .is_some_and(|tier| tier.get() >= source_tier.get())
        })
        .collect();
    let unknown = eligible
        .iter()
        .filter(|v| v.cluster_bytes.is_none())
        .count();
    let max_known = eligible.iter().filter_map(|v| v.cluster_bytes).max();
    let cluster = match (max_known, unknown) {
        (Some(known), 0) => Some(known),
        (Some(known), _) => Some(known.max(4096)),
        (None, 0) => None,
        (None, _) => Some(4096),
    };
    (cluster, unknown)
}

fn require_source(snapshot: &Snapshot, source: &VolumeId) -> EngineResult<()> {
    let v = &snapshot
        .volumes
        .iter()
        .find(|v| v.volume.volume_id == *source)
        .ok_or_else(|| invalid("source_volume_unknown"))?
        .volume;
    if !v.online {
        return Err(invalid("source_offline"));
    }
    if tier_info(v).tier.is_none() {
        return Err(invalid("source_tier_unknown"));
    }
    if v.capacity_bytes.is_none() || v.free_bytes.is_none() {
        return Err(invalid("source_capacity_unknown"));
    }
    Ok(())
}

fn planner_volumes(
    snapshot: &Snapshot,
) -> EngineResult<(Vec<Value>, Vec<PlacementPlanExcludedVolume>)> {
    if snapshot.volumes.len() > 64
        || snapshot.subtrees.len() > 10_000
        || snapshot.root_generations.len() > 64
    {
        return Err(EngineError::ResourceBudget {
            message: "placement snapshot bounds".into(),
        });
    }
    let mut volumes = Vec::new();
    let mut excluded = Vec::new();
    let mut ids = BTreeSet::new();
    for observed in &snapshot.volumes {
        let v = &observed.volume;
        if !ids.insert(v.volume_id.as_str()) {
            return Err(invalid("duplicate volume"));
        }
        let tier = tier_info(v).tier;
        let reason = if !v.online {
            Some(PlacementPlanExcludedVolumeReason::Offline)
        } else if tier.is_none() {
            Some(PlacementPlanExcludedVolumeReason::TierUnknown)
        } else if v.capacity_bytes.is_none() || v.free_bytes.is_none() {
            Some(PlacementPlanExcludedVolumeReason::CapacityUnknown)
        } else {
            None
        };
        if let Some(reason) = reason {
            excluded.push(PlacementPlanExcludedVolume {
                volume_id: v.volume_id.clone(),
                reason,
            });
            continue;
        }
        let capacity = safe(v.capacity_bytes.expect("known capacity").get())?;
        let free = safe(v.free_bytes.expect("known free").get())?;
        let reserve = safe(observed.reserve_bytes.get())?;
        if capacity == 0
            || free > capacity
            || reserve > capacity
            || observed.cluster_bytes == Some(0)
        {
            return Err(invalid("invalid observed volume physics"));
        }
        let mut value = json!({"id": v.volume_id, "capacity_bytes": capacity, "free_bytes": free,
            "reserve_bytes": reserve, "tier": tier.expect("known tier").get(), "online": true});
        if let Some(ro) = v.read_only {
            value["writable"] = json!(!ro);
        }
        volumes.push(value);
    }
    Ok((volumes, excluded))
}

struct Candidates {
    view: PlacementCandidates,
    objects: BTreeMap<(String, [u8; 16]), BTreeSet<String>>,
}

fn shared_groups(candidates: &Candidates) -> BTreeSet<&str> {
    candidates
        .objects
        .values()
        .filter(|ids| ids.len() > 1)
        .flat_map(|ids| ids.iter().map(String::as_str))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn candidates(
    input: &dyn PlacementInput,
    snapshot: &Snapshot,
    source: &VolumeId,
    basis: CandidateBasis,
    max_groups: usize,
    min_bytes: u64,
    cluster: Option<u64>,
    deadline: Instant,
) -> EngineResult<Candidates> {
    let mut node_ids = BTreeSet::new();
    let mut group_ids = BTreeSet::new();
    for s in &snapshot.subtrees {
        if !node_ids.insert(s.node_id.as_str())
            || !group_ids.insert(s.group_id.as_str())
            || s.ancestors.contains(&s.node_id)
        {
            return Err(invalid("duplicate_or_cyclic_subtree"));
        }
    }
    let mut ordered: Vec<_> = snapshot
        .subtrees
        .iter()
        .filter(|s| {
            (basis != CandidateBasis::RootChildren || s.root_child)
                && entry_bytes(s).get() >= min_bytes
                && s.totals.logical_bytes.get() > 0
        })
        .collect();
    ordered.sort_by(|a, b| {
        entry_bytes(b)
            .cmp(&entry_bytes(a))
            .then_with(|| a.group_id.as_str().cmp(b.group_id.as_str()))
    });
    let mut selected: Vec<&Subtree> = Vec::new();
    for s in ordered {
        check_deadline(deadline)?;
        if selected.iter().any(|chosen| {
            chosen.ancestors.contains(&s.node_id) || s.ancestors.contains(&chosen.node_id)
        }) {
            continue;
        }
        selected.push(s);
        if selected.len() == max_groups {
            break;
        }
    }
    let mut groups = Vec::new();
    let mut objects: BTreeMap<(String, [u8; 16]), BTreeSet<String>> = BTreeMap::new();
    for s in selected {
        check_deadline(deadline)?;
        let files = if s.totals.files.get() <= 200_000 {
            input.files(s, &snapshot.root_generations, deadline)?
        } else {
            Vec::new()
        };
        if files.len() > 200_000 {
            return Err(EngineError::ResourceBudget {
                message: "placement files per group exceeds 200000".into(),
            });
        }
        let mut entries = BTreeSet::new();
        let mut unique: BTreeMap<(String, [u8; 16]), (u64, u32, u32)> = BTreeMap::new();
        let mut verified = s.totals.complete
            && s.totals.skipped.get() == 0
            && s.totals.failed.get() == 0
            && s.totals.allocation_unknown_files.get() == 0
            && s.totals.allocated_bytes.is_some()
            && files.len() as u64 == s.totals.files.get();
        let (mut logical, mut allocated, mut destination) = (0u64, 0u64, 0u64);
        for f in &files {
            check_deadline(deadline)?;
            if !entries.insert(f.node_id.as_str()) {
                return Err(invalid("duplicate file entry"));
            }
            logical = logical
                .checked_add(f.logical_bytes)
                .ok_or_else(|| invalid("logical sum overflow"))?;
            allocated = allocated
                .checked_add(f.allocated_bytes.unwrap_or(0))
                .ok_or_else(|| invalid("allocation sum overflow"))?;
            let rounded = match cluster {
                Some(c) if c > 0 => f
                    .logical_bytes
                    .checked_add(c - 1)
                    .and_then(|n| (n / c).checked_mul(c))
                    .ok_or_else(|| invalid("destination rounding overflow"))?,
                _ => f.logical_bytes,
            };
            destination = destination
                .checked_add(rounded)
                .ok_or_else(|| invalid("destination sum overflow"))?;
            let Some(object) = &f.object else {
                verified = false;
                continue;
            };
            let key = (object.volume_serial.clone(), object.file_id);
            objects
                .entry(key.clone())
                .or_default()
                .insert(s.group_id.as_str().into());
            let observed = f.link_observation.as_ref();
            let matches = observed.is_some_and(|o| {
                o.object == *object
                    && o.link_count > 0
                    && f.allocated_bytes.is_some()
                    && o.allocation.parse::<u64>().ok() == f.allocated_bytes
                    && o.size.parse::<u64>().ok() == Some(f.logical_bytes)
                    && !loomward_windows::linkcount::is_placeholder(o.attributes)
                    && o.object.reparse_tag == 0
            });
            let identity_usable = object.file_id != [0; 16]
                && object.reparse_tag == 0
                && snapshot.volumes.iter().any(|v| {
                    v.volume.volume_id == *source
                        && v.volume_serial.as_ref() == Some(&object.volume_serial)
                })
                && (object.quality == IdentityQuality::FileId128
                    || snapshot.volumes.iter().any(|v| {
                        v.volume.volume_id == *source
                            && v.volume
                                .filesystem
                                .as_ref()
                                .is_some_and(|fs| fs.as_str().eq_ignore_ascii_case("NTFS"))
                    }));
            if !matches || !identity_usable {
                verified = false;
                continue;
            }
            let observation = observed.expect("matched observation");
            let allocation = f.allocated_bytes.expect("matched allocation");
            let entry = unique
                .entry(key)
                .or_insert((allocation, observation.link_count, 0));
            if entry.0 != allocation || entry.1 != observation.link_count {
                verified = false;
            }
            entry.2 += 1;
        }
        verified &= logical == s.totals.logical_bytes.get()
            && s.totals.allocated_bytes == Some(Bytes(allocated))
            && unique.values().all(|(_, links, names)| links == names);
        let relief = if verified {
            Some(Bytes(
                unique
                    .values()
                    .try_fold(0u64, |sum, (allocation, _, _)| sum.checked_add(*allocation))
                    .ok_or_else(|| invalid("relief sum overflow"))?,
            ))
        } else {
            None
        };
        let coverage = if s.totals.complete
            && files.len() as u64 == s.totals.files.get()
            && s.totals.skipped.get() == 0
            && s.totals.failed.get() == 0
        {
            CoverageState::Complete
        } else {
            CoverageState::Partial
        };
        groups.push(CandidateGroup {
            group_id: s.group_id.clone(),
            node_id: s.node_id.clone(),
            root_id: s.root_id.clone(),
            name: s.name.clone(),
            source_bytes: entry_bytes(s),
            destination_bytes: Bytes(if coverage == CoverageState::Complete {
                destination
            } else {
                s.totals.logical_bytes.get()
            }),
            transfer_bytes: s.totals.logical_bytes,
            estimate_basis: if s.totals.allocated_bytes.is_some()
                && s.totals.allocation_unknown_files.get() == 0
            {
                CandidateGroupEstimateBasis::AllocatedEntries
            } else {
                CandidateGroupEstimateBasis::LogicalFallback
            },
            estimated_relief_bytes: relief,
            relief_basis: if relief.is_some() {
                CandidateGroupReliefBasis::VerifiedUniqueAllocation
            } else {
                CandidateGroupReliefBasis::Unknown
            },
            heat: None,
            heat_basis: HeatBasis::Unknown,
            newest_modified_at: s.newest_modified_at.clone(),
            pinned: s.pinned,
            active: s.active,
            protected: s.protected,
            days_since_move: s.days_since_move,
            coverage,
        });
    }
    check_deadline(deadline)?;
    Ok(Candidates {view: PlacementCandidates {source_volume_id: source.clone(), root_generations: bounded(BoundedVec::new(snapshot.root_generations.clone()))?,
        groups: bounded(BoundedVec::new(groups))?, note: Text::new("Default-stream entry estimates and point-in-time verified relief; simulation only.").expect("bounded note")}, objects})
}

fn entry_bytes(s: &Subtree) -> Bytes {
    if s.totals.allocation_unknown_files.get() == 0 {
        s.totals.allocated_bytes.unwrap_or(s.totals.logical_bytes)
    } else {
        s.totals.logical_bytes
    }
}

#[cfg(test)]
mod tests;
