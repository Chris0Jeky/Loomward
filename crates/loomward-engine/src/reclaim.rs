//! Reclaim plan (LW-131; docs/52 sections 4 and 5, lane R1c): a read-only grouping of developer
//! clutter (LW-119, rules v1) and machine-store advice (rules v2) for one volume, with
//! per-candidate reclaimable bytes from placement's verified-relief rule (LW-108).
//!
//! Nothing here executes. `owner_step` is display text for the owner to run in the owning tool,
//! `relocation` names a variable or setting the owner may change, and no field, command or
//! function performs a file or process effect (AGENTS.md invariant 1). Unknown stays `None` with
//! a reason, never zero (invariant 4). Candidates may nest (a project output inside a worktree),
//! so the plan never sums them.

use crate::{
    placement::{FileEntry, ReliefTally},
    EngineError, EngineResult,
};
use loomward_catalog::{DeveloperClutter, MachineStores, NodeKey};
use loomward_windows::{
    linkcount::{Stop, MAX_FILES_PER_GROUP},
    recycle_bin::RecycleBinSize,
};
use serde::Serialize;
use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashMap, HashSet},
};

/// The plan's recogniser rules version (docs/52 section 3).
pub const RULES_VERSION: u32 = 3;
/// Reason for unknown relief when the link-count pass did not verify every identity.
pub const RELIEF_UNOBSERVED: &str = "hard links outside the candidate, or not observed";
const SIZES_UNKNOWN: &str =
    "sizes unknown: the subtree is partial, stale, mid-scan or has unknown allocation";
const VIRTUAL_DISK: &str =
    "guest free space is not observable from the host without the owning tool";
const SYSTEM_MANAGED: &str = "managed by Windows; never counted as reclaimable";
const BIN_STEP: &str =
    "Emptying the Recycle Bin in Explorer is the owner's step after reviewing its contents.";
const BIN_UNCERTAINTY: &str = "Recycling on this volume frees nothing here until the bin is emptied, and its contents and recoverability are unverified. This is advice, never a disposability claim.";
const DEVELOPER_UNCERTAINTY: &str = "Names only: contents may contain unique source or datasets. Prerequisites, offline availability, active builds and rebuild cost are unknown. This is advice, never a disposability claim.";

/// Observed physics of the volume under pressure; unknown stays `None`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ReclaimVolume {
    /// Catalogue volume key.
    pub volume_key: String,
    /// Display label.
    pub label: Option<String>,
    /// Device bus hint, not a benchmark.
    pub bus_type: Option<String>,
    /// Observed capacity.
    pub capacity_bytes: Option<u64>,
    /// Observed free space.
    pub free_bytes: Option<u64>,
    /// The owner's reserve preference.
    pub reserve_bytes: Option<u64>,
    /// Native serial binding file identities to this volume; unknown prevents verified relief.
    #[serde(skip)]
    pub volume_serial: Option<String>,
    /// Whether the volume is NTFS (legacy 64-bit identities are usable only there).
    #[serde(skip)]
    pub ntfs: bool,
}

/// One granted, scanned root on the volume. Callers pass disjoint (outermost) roots.
#[derive(Debug, Clone, Serialize)]
pub struct CoveredRoot {
    /// Catalogue root ID.
    pub root_id: i64,
    /// Catalogue coverage of the root.
    pub coverage: String,
    /// Allocated bytes of the whole root, unknown when not observed.
    pub allocated_bytes: Option<u64>,
}

/// The budgeted link-count pass over one candidate's names (`loomward_windows::linkcount`).
#[derive(Debug, Clone)]
pub struct CandidateFiles {
    /// Every name the catalogue lists inside the candidate, with its link observation.
    pub files: Vec<FileEntry>,
    /// Why the pass stopped early; any stop leaves relief unknown.
    pub stop: Option<Stop>,
}

/// Everything the plan composes. No field is a grant or an effect operand.
#[derive(Debug, Clone, Default)]
pub struct ReclaimInput {
    /// The volume under pressure.
    pub volume: ReclaimVolume,
    /// Granted roots on that volume.
    pub covered_roots: Vec<CoveredRoot>,
    /// `Reader::developer_clutter` per covered root, read `without_repairs()` (the reclaim read requests no write).
    pub developer: Vec<DeveloperClutter>,
    /// `Reader::machine_stores` per covered root.
    pub machine: Vec<MachineStores>,
    /// Link-count pass per candidate node; a missing entry means the pass has not run.
    pub files: HashMap<NodeKey, CandidateFiles>,
    /// Verified relief `reclaim.observe` stored for this catalogue generation (docs/52 section 6.1),
    /// already computed by [`reclaimable`] from the same candidate; unknown results are absent.
    pub observed: HashMap<NodeKey, u64>,
    /// `loomward_windows::recycle_bin::query`, made only for a volume-root grant.
    pub recycle_bin: Option<RecycleBinSize>,
    /// The volume the owner might relocate stores to (docs/07 tiers), when one is known.
    pub relocation_target_volume: Option<String>,
    /// Candidates per group, 1..=200.
    pub limit: usize,
}

/// What the owner does next (docs/52 section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    /// Generated or cache data whose owning tool offers a supported cleanup command.
    ToolOwnedCleanup,
    /// Active caches and model stores better placed on another volume.
    Relocate,
    /// Not proven generated; belongs to the archive plan, never to a cleanup list.
    ArchiveCandidates,
    /// System-managed space, the Recycle Bin and virtual disks with unknown relief.
    Informational,
}

/// A variable or tool setting the owner may change; Loomward never changes it.
#[derive(Debug, Clone, Serialize)]
pub struct Relocation {
    /// Environment variable or tool setting name.
    pub variable_or_setting: &'static str,
    /// Suggested destination volume, when known.
    pub suggested_target_volume: Option<String>,
}

/// One candidate rendered as facts, never as a verdict.
#[derive(Debug, Clone, Serialize)]
pub struct ReclaimCandidate {
    /// Catalogue reference, absent for the Windows-reported Recycle Bin.
    pub node_id: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Candidate class (docs/52 section 3).
    pub class: &'static str,
    /// Owning tool.
    pub provider: &'static str,
    /// Conventional-name kind for project clutter.
    pub kind: Option<&'static str>,
    /// Observed logical bytes.
    pub logical_bytes: Option<u64>,
    /// Observed allocated bytes (the container size for a virtual disk).
    pub allocated_bytes: Option<u64>,
    /// Verified unique allocation that removing the candidate would free.
    pub reclaimable_bytes: Option<u64>,
    /// Why `reclaimable_bytes` is unknown.
    pub reclaimable_reason: Option<&'static str>,
    /// Catalogue coverage.
    pub coverage: String,
    /// Display text only: the owner runs it in the owning tool.
    pub owner_step: &'static str,
    /// Where the owner could point the tool instead.
    pub relocation: Option<Relocation>,
    /// Observed facts behind the candidate.
    pub evidence: Vec<String>,
    /// What stays unverified.
    pub uncertainty: &'static str,
}

/// One group, ranked by reclaimable bytes with unknowns last.
#[derive(Debug, Clone, Serialize)]
pub struct ReclaimGroup {
    /// The owner's next step.
    pub kind: GroupKind,
    /// At most `limit` candidates.
    pub candidates: Vec<ReclaimCandidate>,
}

/// The read-only plan (docs/52 section 5, "Proposed contract").
#[derive(Debug, Clone, Serialize)]
pub struct ReclaimPlan {
    /// The volume under pressure.
    pub volume: ReclaimVolume,
    /// Granted roots on the volume.
    pub covered_roots: Vec<CoveredRoot>,
    /// Always the four groups, in [`GroupKind`] order.
    pub groups: Vec<ReclaimGroup>,
    /// Volume used minus the covered roots, unknown when not computable.
    pub unaccounted_bytes: Option<u64>,
    /// Recogniser rules version.
    pub rules_version: u32,
    /// Some input was partial, stale or mid-scan.
    pub partial: bool,
    /// Some input or group hit its cap.
    pub truncated: bool,
    /// The machine-store walk ran out of work budget.
    pub budget_hit: bool,
}

/// Build the plan from catalogue reads and link observations. Pure: no I/O, no effects.
pub fn plan(input: ReclaimInput) -> EngineResult<ReclaimPlan> {
    let invalid = |message: &str| EngineError::InvalidRequest {
        message: message.into(),
    };
    if !(1..=200).contains(&input.limit) {
        return Err(invalid("reclaim limit"));
    }
    if input.developer.iter().any(|r| r.rules_version != 1)
        || input
            .machine
            .iter()
            .any(|r| r.rules_version != RULES_VERSION)
    {
        return Err(invalid("reclaim rules version"));
    }
    let mut partial = false;
    let mut truncated = false;
    let mut budget_hit = false;
    let mut seen = HashSet::new();
    let mut groups: Vec<(GroupKind, Vec<ReclaimCandidate>)> = [
        GroupKind::ToolOwnedCleanup,
        GroupKind::Relocate,
        GroupKind::ArchiveCandidates,
        GroupKind::Informational,
    ]
    .map(|kind| (kind, Vec::new()))
    .into();
    let mut push = |kind: GroupKind, candidate: ReclaimCandidate| {
        groups
            .iter_mut()
            .find(|(k, _)| *k == kind)
            .expect("four groups")
            .1
            .push(candidate);
    };
    let relocation = |variable_or_setting| Relocation {
        variable_or_setting,
        suggested_target_volume: input.relocation_target_volume.clone(),
    };

    for report in &input.developer {
        partial |= report.partial;
        truncated |= report.truncated;
        for c in &report.candidates {
            if !seen.insert(c.directory) {
                continue;
            }
            let (reclaimable, reason) = reclaimable(
                "project_clutter",
                c.logical_bytes,
                c.allocated_bytes,
                input.files.get(&c.directory),
                input.observed.get(&c.directory).copied(),
                &input.volume,
            )?;
            let owner_step = match c.provider {
                "cargo" => "Use cargo clean in the owning project after review; preview it with cargo clean --dry-run.",
                "dotnet" => "Use dotnet clean in the owning project after review.",
                "gradle" => "Use the project's gradlew clean after review.",
                "npm" => "After review, remove and reinstall from the project's lockfile with its package manager.",
                "python" => "After review, recreate the environment from its declared dependencies, or let Python regenerate bytecode.",
                _ => return Err(invalid("developer clutter provider")),
            };
            push(
                GroupKind::ToolOwnedCleanup,
                ReclaimCandidate {
                    node_id: Some(c.directory.reference()),
                    name: Some(c.directory_name.clone()),
                    class: "project_clutter",
                    provider: c.provider,
                    kind: Some(c.kind),
                    logical_bytes: c.logical_bytes,
                    allocated_bytes: c.allocated_bytes,
                    reclaimable_bytes: reclaimable,
                    reclaimable_reason: reason,
                    coverage: c.coverage.clone(),
                    owner_step,
                    relocation: (c.provider == "cargo" && c.kind == "build_output")
                        .then(|| relocation("CARGO_TARGET_DIR")),
                    evidence: vec![
                        format!("{} Anchor: {}.", c.reason, c.anchor_name),
                        c.regeneration_hint.to_string(),
                        format!("Regeneration cost: {}.", c.regeneration_cost),
                    ],
                    uncertainty: DEVELOPER_UNCERTAINTY,
                },
            );
        }
    }

    let mut bin_listed = false;
    for report in &input.machine {
        partial |= report.partial;
        truncated |= report.truncated;
        budget_hit |= report.budget_hit;
        for c in &report.candidates {
            if !seen.insert(c.node) {
                continue;
            }
            let (reclaimable, reason) = reclaimable(
                c.class,
                c.logical_bytes,
                c.allocated_bytes,
                input.files.get(&c.node),
                input.observed.get(&c.node).copied(),
                &input.volume,
            )?;
            let mut evidence = vec![c.reason.to_string()];
            if c.class == "virtual_disk" {
                evidence.push("Allocated bytes are the container size.".into());
            }
            if c.class == "recycle_bin" {
                bin_listed = true;
                evidence.extend(input.recycle_bin.map(bin_fact));
            }
            let kind = match (c.class, c.provider) {
                ("shared_build_output", _) => GroupKind::ToolOwnedCleanup,
                // Cargo's cache cleanup tools are third-party: no supported command.
                ("package_cache", "cargo") => GroupKind::Relocate,
                ("package_cache", _) => GroupKind::ToolOwnedCleanup,
                ("model_store", _) => GroupKind::Relocate,
                // Steam's supported step is Move install folder; uninstall stays the owner's call.
                ("game_library", _) => GroupKind::Relocate,
                // A checkout may hold unique work: never on the cleanup list.
                ("agent_worktree", _) => GroupKind::ArchiveCandidates,
                _ => GroupKind::Informational,
            };
            push(
                kind,
                ReclaimCandidate {
                    node_id: Some(c.node.reference()),
                    name: Some(c.name.clone()),
                    class: c.class,
                    provider: c.provider,
                    kind: None,
                    logical_bytes: c.logical_bytes,
                    allocated_bytes: c.allocated_bytes,
                    reclaimable_bytes: reclaimable,
                    reclaimable_reason: reason,
                    coverage: c.coverage.clone(),
                    owner_step: c.owner_step,
                    relocation: c.relocation.map(relocation),
                    evidence,
                    uncertainty: c.uncertainty,
                },
            );
        }
    }
    if let (false, Some(bin)) = (bin_listed, input.recycle_bin) {
        push(
            GroupKind::Informational,
            ReclaimCandidate {
                node_id: None,
                name: None,
                class: "recycle_bin",
                provider: "windows",
                kind: None,
                logical_bytes: Some(bin.bytes),
                allocated_bytes: None,
                reclaimable_bytes: None,
                reclaimable_reason: Some(RELIEF_UNOBSERVED),
                coverage: "unknown".into(),
                owner_step: BIN_STEP,
                relocation: None,
                evidence: vec![bin_fact(bin)],
                uncertainty: BIN_UNCERTAINTY,
            },
        );
    }

    let groups = groups
        .into_iter()
        .map(|(kind, mut candidates)| {
            candidates.sort_by_key(|c| {
                (
                    c.reclaimable_bytes.is_none(),
                    Reverse(c.reclaimable_bytes),
                    c.allocated_bytes.is_none(),
                    Reverse(c.allocated_bytes),
                    c.node_id.clone(),
                )
            });
            truncated |= candidates.len() > input.limit;
            candidates.truncate(input.limit);
            ReclaimGroup { kind, candidates }
        })
        .collect();
    Ok(ReclaimPlan {
        unaccounted_bytes: unaccounted(&input.volume, &input.covered_roots),
        volume: input.volume,
        covered_roots: input.covered_roots,
        groups,
        rules_version: RULES_VERSION,
        partial,
        truncated,
        budget_hit,
    })
}

fn bin_fact(bin: RecycleBinSize) -> String {
    format!(
        "Windows reports {} bytes in {} recycled items on this volume.",
        bin.bytes, bin.items
    )
}

/// Volume used minus the covered roots; unknown unless every number is observed and complete.
fn unaccounted(volume: &ReclaimVolume, roots: &[CoveredRoot]) -> Option<u64> {
    let used = volume.capacity_bytes?.checked_sub(volume.free_bytes?)?;
    let covered = roots.iter().try_fold(0u64, |sum, root| {
        (root.coverage == "complete")
            .then_some(root.allocated_bytes)
            .flatten()
            .and_then(|bytes| sum.checked_add(bytes))
    })?;
    // More covered than used (double-counted links, overlapping roots) is not an honest zero.
    used.checked_sub(covered)
}

/// docs/52 section 4: placement's verified-relief rule with the candidate as the deletion set.
/// `observed` is a verdict this function already returned for the same candidate and generation.
pub fn reclaimable(
    class: &str,
    logical: Option<u64>,
    allocated: Option<u64>,
    files: Option<&CandidateFiles>,
    observed: Option<u64>,
    volume: &ReclaimVolume,
) -> EngineResult<(Option<u64>, Option<&'static str>)> {
    let unknown = |reason| Ok((None, Some(reason)));
    match class {
        "virtual_disk" => return unknown(VIRTUAL_DISK),
        "system_managed" => return unknown(SYSTEM_MANAGED),
        _ => {}
    }
    let (Some(logical), Some(allocated)) = (logical, allocated) else {
        return unknown(SIZES_UNKNOWN);
    };
    if let (None, Some(bytes)) = (files, observed) {
        return Ok((Some(bytes), None));
    }
    let Some(files) = files.filter(|f| f.stop.is_none() && f.files.len() <= MAX_FILES_PER_GROUP)
    else {
        return unknown(RELIEF_UNOBSERVED);
    };
    let mut names = BTreeSet::new();
    let mut tally = ReliefTally::default();
    let (mut logical_seen, mut allocated_seen) = (0u64, 0u64);
    for f in &files.files {
        // A repeated name would inflate the names seen and could hide an outside link.
        if !names.insert(f.node_id.as_str()) {
            return unknown(RELIEF_UNOBSERVED);
        }
        // An overflowing sum cannot match a real total: unknown for this candidate, never a
        // saturated match that fails the whole plan later (#954).
        let (Some(l), Some(a)) = (
            logical_seen.checked_add(f.logical_bytes),
            allocated_seen.checked_add(f.allocated_bytes.unwrap_or(0)),
        ) else {
            return unknown(RELIEF_UNOBSERVED);
        };
        (logical_seen, allocated_seen) = (l, a);
        tally.add(f, volume.volume_serial.as_deref(), volume.ntfs);
    }
    // The names must cover the candidate's own totals, or some were not seen.
    if logical_seen != logical || allocated_seen != allocated {
        return unknown(RELIEF_UNOBSERVED);
    }
    Ok(match tally.relief()? {
        Some(bytes) => (Some(bytes), None),
        None => (None, Some(RELIEF_UNOBSERVED)),
    })
}
