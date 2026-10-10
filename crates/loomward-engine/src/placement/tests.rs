use super::*;
use loomward_core::{planner::Scenario, planner_v2::plan_v2};
use loomward_windows::identity::IdentityQuality;
use serde_json::{from_value, json};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Clone)]
struct MemoryInput {
    snapshot: Snapshot,
    files: BTreeMap<String, Vec<FileEntry>>,
}

impl PlacementInput for MemoryInput {
    fn snapshot(&self, _: Option<&VolumeId>, _: Instant) -> EngineResult<Snapshot> {
        Ok(self.snapshot.clone())
    }
    fn files(
        &self,
        subtree: &Subtree,
        generations: &[RootGeneration],
        _: Instant,
    ) -> EngineResult<Vec<FileEntry>> {
        assert_eq!(generations, self.snapshot.root_generations);
        Ok(self
            .files
            .get(subtree.group_id.as_str())
            .cloned()
            .unwrap_or_default())
    }
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn request() -> PlacementSimulateRequest {
    from_value(json!({
        "source_volume_id": "vo_source", "target_free_bytes": "100",
        "max_transfer_bytes": "1000", "heat_policy": "unknown_is_ineligible",
        "relief_policy": "verified_only", "candidate_basis": "largest_dirs",
        "max_groups": 200, "overrides": [{"group_id": "cg_one", "heat": 0.1}],
        "node_budget": 50000, "save": false
    }))
    .unwrap()
}

fn volume(id: &str, capacity: u64, free: u64, tier: i64) -> VolumeObservation {
    VolumeObservation {
        volume: from_value(json!({
            "volume_id": id, "display_name": id, "mount_points": [], "filesystem": "NTFS",
            "label": null, "online": true, "read_only": false, "removable": false,
            "capacity_bytes": capacity.to_string(), "free_bytes": free.to_string(),
            "device": {"bus_type": "nvme", "seek_penalty": false, "multiple_disks": false,
                       "basis": "ioctl_storage_query_property"},
            "tier": {"tier": tier, "basis": "declared", "declared_tier": tier,
                     "hint_tier": 1, "note": "test"},
            "features": {"file_ids_128": true, "hard_links": true, "sparse_files": true,
                         "compression": true, "reparse_points": true, "usn_journal": true},
            "observed_at": "2026-10-10T00:00:00Z"
        }))
        .unwrap(),
        reserve_bytes: Bytes(0),
        cluster_bytes: Some(64),
        volume_serial: Some("1".into()),
    }
}

fn subtree(id: &str) -> Subtree {
    Subtree {
        group_id: GroupId::new(format!("cg_{id}")).unwrap(),
        node_id: NodeId::new(format!("nd_{id}")).unwrap(),
        root_id: RootId::new("rt_one").unwrap(),
        name: Text::new(id).unwrap(),
        ancestors: vec![],
        root_child: true,
        totals: from_value(json!({"files": 2, "dirs": 1, "logical_bytes": "200",
            "allocated_bytes": "200", "allocation_unknown_files": 0, "skipped": 0,
            "failed": 0, "complete": true, "stream_coverage": "default_stream_only",
            "unique_objects": 1, "unique_allocated_bytes": "100", "multi_link_entries": 2
        }))
        .unwrap(),
        newest_modified_at: None,
        modified_age_days: None,
        pinned: Some(false),
        active: Some(false),
        protected: Some(false),
        days_since_move: Some(Int::new(7).unwrap()),
    }
}

fn file(name: &str) -> FileEntry {
    let object = ObjectKey {
        volume_serial: "1".into(),
        file_id: [1; 16],
        quality: IdentityQuality::FileId128,
        creation_time: "1".into(),
        reparse_tag: 0,
    };
    FileEntry {
        node_id: NodeId::new(format!("nd_{name}")).unwrap(),
        logical_bytes: 100,
        allocated_bytes: Some(100),
        object: Some(object.clone()),
        link_observation: Some(ObservedIdentity {
            object,
            link_count: 2,
            attributes: 0,
            size: "100".into(),
            allocation: "100".into(),
            last_access_time: "1".into(),
            last_write_time: "1".into(),
            change_time: "1".into(),
            ancestors: vec![],
        }),
    }
}

fn input() -> MemoryInput {
    MemoryInput {
        snapshot: Snapshot {
            volumes: vec![
                volume("vo_source", 100, 0, 1),
                volume("vo_target", 1000, 1000, 2),
            ],
            subtrees: vec![subtree("one")],
            root_generations: vec![
                from_value(json!({"root_id": "rt_one", "generation": "1"})).unwrap()
            ],
        },
        files: BTreeMap::from([("cg_one".into(), vec![file("a"), file("b")])]),
    }
}

#[test]
fn placement_relief_maps_entry_200_to_unique_100_and_planner_accepts() {
    let built = build_scenario(&input(), &request(), deadline()).unwrap();
    let candidate = &built.candidates.groups[0];
    assert_eq!(candidate.source_bytes, Bytes(200));
    assert_eq!(candidate.estimated_relief_bytes, Some(Bytes(100)));
    assert_eq!(
        candidate.relief_basis,
        CandidateGroupReliefBasis::VerifiedUniqueAllocation
    );
    assert_eq!(candidate.destination_bytes, Bytes(256));
    assert_eq!(candidate.transfer_bytes, Bytes(200));
    assert_eq!(built.scenario["groups"][0]["source_bytes"], 100);
    let scenario: Scenario = from_value(built.scenario).unwrap();
    let plan = plan_v2(&scenario, 50000).unwrap();
    assert!(plan.satisfied);
    assert_eq!(plan.proposals[0].source_bytes_relieved, 100);
}

#[test]
fn placement_antichain_property_on_random_trees_and_root_children() {
    let mut seed = 108u64;
    for _ in 0..100 {
        let mut input = input();
        input.snapshot.volumes[0] = volume("vo_source", 100000, 0, 1);
        input.snapshot.subtrees.clear();
        input.files.clear();
        for i in 0..60usize {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut s = subtree(&format!("tree_{i}"));
            if i > 0 && seed % 4 != 0 {
                let parent = &input.snapshot.subtrees[(seed as usize >> 8) % i];
                s.ancestors = parent.ancestors.clone();
                s.ancestors.push(parent.node_id.clone());
                s.root_child = false;
            }
            let mut files = vec![file(&format!("tree_{i}_a")), file(&format!("tree_{i}_b"))];
            for f in &mut files {
                f.object.as_mut().unwrap().file_id = (i as u128 + 1).to_le_bytes();
                f.link_observation.as_mut().unwrap().object = f.object.clone().unwrap();
            }
            input.files.insert(s.group_id.as_str().into(), files);
            input.snapshot.subtrees.push(s);
        }
        let mut req = request();
        req.overrides = BoundedVec::new(vec![]).unwrap();
        for basis in [CandidateBasis::LargestDirs, CandidateBasis::RootChildren] {
            req.candidate_basis = basis;
            let built = build_scenario(&input, &req, deadline()).unwrap();
            let selected = &built.candidates.groups;
            assert!(!selected.is_empty());
            for a in selected.iter() {
                let node = input
                    .snapshot
                    .subtrees
                    .iter()
                    .find(|s| s.node_id == a.node_id)
                    .unwrap();
                if basis == CandidateBasis::RootChildren {
                    assert!(node.root_child);
                }
                for b in selected.iter() {
                    assert!(!node.ancestors.contains(&b.node_id));
                }
            }
        }
    }
}

#[test]
fn placement_pre_rejects_unknown_relief_shared_objects_and_incomplete_coverage() {
    let mut input = input();
    input.files.get_mut("cg_one").unwrap()[0].link_observation = None;
    let built = build_scenario(&input, &request(), deadline()).unwrap();
    assert_eq!(
        built.pre_rejected[0].reason,
        PlacementPlanPreRejectedReason::ReliefUnknown
    );
    assert!(built.scenario["groups"].as_array().unwrap().is_empty());
    input.snapshot.subtrees[0].totals.complete = false;
    let built = build_scenario(&input, &request(), deadline()).unwrap();
    assert_eq!(
        built.pre_rejected[0].reason,
        PlacementPlanPreRejectedReason::GroupCoverageIncomplete
    );
    input = super::tests::input();
    input.snapshot.subtrees.push(subtree("two"));
    input
        .files
        .insert("cg_two".into(), vec![file("c"), file("d")]);
    let built = build_scenario(&input, &request(), deadline()).unwrap();
    assert_eq!(built.pre_rejected.len(), 2);
    assert!(built
        .pre_rejected
        .iter()
        .all(|r| r.reason == PlacementPlanPreRejectedReason::SharesObjectsWithOtherGroup));
    let mut req = request();
    req.relief_policy = PlacementSimulateRequestReliefPolicy::EntryAllocationWhatif;
    assert_eq!(
        build_scenario(&input, &req, deadline())
            .unwrap()
            .pre_rejected
            .len(),
        2
    );
}

#[test]
fn placement_unknowns_are_omitted_never_null_and_planner_reasons_stay_exact() {
    let mut input = input();
    input.snapshot.subtrees[0].pinned = None;
    input.snapshot.subtrees[0].active = None;
    input.snapshot.subtrees[0].protected = None;
    input.snapshot.subtrees[0].days_since_move = None;
    let mut req = request();
    req.overrides = BoundedVec::new(vec![]).unwrap();
    let built = build_scenario(&input, &req, deadline()).unwrap();
    let group = &built.scenario["groups"][0];
    for key in ["pinned", "active", "protected", "heat", "days_since_move"] {
        assert!(group.get(key).is_none(), "{key}");
    }
    assert_eq!(built.candidates.groups[0].pinned, None);
    let plan = plan_v2(&from_value(built.scenario).unwrap(), 50000).unwrap();
    assert_eq!(
        plan.rejected[0].reason,
        "pinned_active_protected_or_unspecified"
    );
    input.snapshot.subtrees[0].pinned = Some(false);
    input.snapshot.subtrees[0].active = Some(false);
    input.snapshot.subtrees[0].protected = Some(false);
    let plan = plan_v2(
        &from_value(build_scenario(&input, &req, deadline()).unwrap().scenario).unwrap(),
        50000,
    )
    .unwrap();
    assert_eq!(plan.rejected[0].reason, "heat_unknown");
}

#[test]
fn placement_requires_known_allocation_matching_identity_and_all_names() {
    for mode in 0..8 {
        let mut input = input();
        let file = &mut input.files.get_mut("cg_one").unwrap()[0];
        match mode {
            0 => file.object = None,
            1 => file.allocated_bytes = None,
            2 => file.link_observation.as_mut().unwrap().object.file_id = [9; 16],
            3 => file.link_observation.as_mut().unwrap().allocation = "unknown".into(),
            4 => file.link_observation.as_mut().unwrap().link_count = 3,
            5 => file.link_observation.as_mut().unwrap().attributes = 0x400000,
            6 => file.link_observation.as_mut().unwrap().size = "101".into(),
            _ => {
                input.snapshot.subtrees[0].totals.allocated_bytes = None;
                input.snapshot.subtrees[0].totals.allocation_unknown_files = Count::new(1).unwrap();
            }
        }
        let built = build_scenario(&input, &request(), deadline()).unwrap();
        assert_eq!(
            built.candidates.groups[0].estimated_relief_bytes, None,
            "mode {mode}"
        );
        assert_eq!(
            built.pre_rejected[0].reason,
            PlacementPlanPreRejectedReason::ReliefUnknown,
            "mode {mode}"
        );
    }
}

#[test]
fn placement_whatif_uses_entry_allocation_and_labels_assumptions() {
    let mut input = input();
    input.snapshot.volumes[0] = volume("vo_source", 200, 0, 1);
    input.files.get_mut("cg_one").unwrap()[0].link_observation = None;
    let mut req = request();
    req.relief_policy = PlacementSimulateRequestReliefPolicy::EntryAllocationWhatif;
    let built = build_scenario(&input, &req, deadline()).unwrap();
    assert!(built.pre_rejected.is_empty());
    assert_eq!(built.scenario["groups"][0]["source_bytes"], 200);
    assert_eq!(built.candidates.groups[0].estimated_relief_bytes, None);
    assert!(built
        .assumptions
        .iter()
        .any(|s| s.as_str().contains("assumed reclaimable")));
    assert!(
        plan_v2(&from_value(built.scenario).unwrap(), 50000)
            .unwrap()
            .satisfied
    );
}

#[test]
fn placement_excludes_unknown_or_offline_volumes_but_retains_them_in_tier_view() {
    let mut input = input();
    let mut unknown = volume("vo_unknown", 100, 100, 3);
    unknown.volume.tier.declared_tier = None;
    unknown.volume.device.seek_penalty = None;
    input.snapshot.volumes.push(unknown);
    let mut offline = volume("vo_offline", 100, 100, 3);
    offline.volume.online = false;
    input.snapshot.volumes.push(offline);
    let mut missing = volume("vo_missing", 100, 100, 3);
    missing.volume.capacity_bytes = None;
    input.snapshot.volumes.push(missing);
    let built = build_scenario(&input, &request(), deadline()).unwrap();
    assert_eq!(
        built
            .excluded_volumes
            .iter()
            .map(|e| e.reason)
            .collect::<Vec<_>>(),
        vec![
            PlacementPlanExcludedVolumeReason::TierUnknown,
            PlacementPlanExcludedVolumeReason::Offline,
            PlacementPlanExcludedVolumeReason::CapacityUnknown
        ]
    );
    assert_eq!(built.scenario["volumes"].as_array().unwrap().len(), 2);
    input.snapshot.volumes[0].volume.online = false;
    assert!(matches!(
        build_scenario(&input, &request(), deadline()),
        Err(EngineError::InvalidRequest { .. })
    ));
}

#[test]
fn placement_deadline_before_and_after_input_is_an_error() {
    assert!(matches!(
        build_scenario(&input(), &request(), Instant::now()),
        Err(EngineError::DeadlineExceeded { .. })
    ));
    struct Slow;
    impl PlacementInput for Slow {
        fn snapshot(&self, _: Option<&VolumeId>, deadline: Instant) -> EngineResult<Snapshot> {
            while Instant::now() < deadline {
                std::thread::yield_now();
            }
            Ok(input().snapshot)
        }
        fn files(
            &self,
            _: &Subtree,
            _: &[RootGeneration],
            _: Instant,
        ) -> EngineResult<Vec<FileEntry>> {
            unreachable!()
        }
    }
    assert!(matches!(
        build_scenario(&Slow, &request(), Instant::now() + Duration::from_millis(1)),
        Err(EngineError::DeadlineExceeded { .. })
    ));
}

/// Checks planner parity on builder scenarios: builder-produced scenarios must
/// match the Python reference planner field by field. It does not validate
/// the builder itself.
#[test]
fn placement_planner_parity_on_builder_scenarios() {
    use std::process::Command;
    match Command::new("py").args(["-3", "--version"]).output() {
        Ok(o) if o.status.success() => (),
        _ => {
            eprintln!("SKIP: placement planner parity on builder scenarios requires Python via py -3");
            return;
        }
    }
    let mut cases = Vec::new();
    for mode in 0..10 {
        let mut input = input();
        let mut req = request();
        match mode {
            1 => req.overrides = BoundedVec::new(vec![]).unwrap(),
            2 => input.snapshot.subtrees[0].pinned = None,
            3 => input.files.get_mut("cg_one").unwrap()[0].link_observation = None,
            4 => input.snapshot.subtrees[0].days_since_move = Some(Int::new(0).unwrap()),
            5 => {
                req.max_transfer_bytes = Bytes(1);
            }
            6 => input.snapshot.volumes[0].volume.read_only = Some(true),
            7 => input.snapshot.volumes[1].reserve_bytes = Bytes(1000),
            8 => req.overrides = from_value(json!([{"group_id":"cg_one","heat":0.9}])).unwrap(),
            9 => {
                input.snapshot.volumes[0] = volume("vo_source", 200, 0, 1);
                input.snapshot.subtrees.push(subtree("two"));
                let mut files = vec![file("c"), file("d")];
                for f in &mut files {
                    f.object.as_mut().unwrap().file_id = [2; 16];
                    f.link_observation.as_mut().unwrap().object = f.object.clone().unwrap();
                }
                input.files.insert("cg_two".into(), files);
                req.target_free_bytes = Bytes(150);
                req.overrides = from_value(
                    json!([{"group_id":"cg_one","heat":0.1},{"group_id":"cg_two","heat":0.1}]),
                )
                .unwrap();
            }
            _ => (),
        }
        for budget in [0, 1, 50000] {
            req.node_budget = Int::new(budget).unwrap();
            let built = build_scenario(&input, &req, deadline()).unwrap();
            let scenario: Scenario = from_value(built.scenario.clone()).unwrap();
            let core = plan_v2(&scenario, budget as u64).unwrap();
            let wire = engine()
                .placement_simulate(&input, &req, deadline(), None)
                .unwrap();
            assert_eq!(
                serde_json::to_vec(&wire.rejected).unwrap(),
                serde_json::to_vec(&core.rejected).unwrap()
            );
            cases.push(json!({"scenario": built.scenario, "node_budget": budget,
                "expected": core}));
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let fixture = temp.path().join("placement-builder-parity.json");
    std::fs::write(&fixture, serde_json::to_vec(&cases).unwrap()).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = Command::new("py").args(["-3","-c",
        "import json,sys; sys.path.insert(0,'python'); from loomward.planner_v2 import plan_tiers_v2; cases=json.load(open(sys.argv[1]));\nfor c in cases:\n actual=plan_tiers_v2(c['scenario'],node_budget=c['node_budget']); assert actual == c['expected'], (actual,c['expected'])\nprint(f'{len(cases)} builder scenarios match Python field by field')"])
        .arg(fixture).current_dir(root).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("30 builder scenarios"));
}

fn engine() -> Engine {
    Engine::open(crate::EngineConfig {
        state_dir: "unused".into(),
        dataset_class: DatasetClass::Synthetic,
    })
    .unwrap()
}

#[derive(Default)]
struct MemoryStore(std::sync::Mutex<Vec<ProposalDetail>>);

impl ProposalStore for MemoryStore {
    fn save(
        &self,
        source: &VolumeId,
        digest: &Digest,
        plan: &PlacementPlan,
        deadline: Instant,
    ) -> EngineResult<PlacementPlan> {
        check_deadline(deadline)?;
        let mut records = self.0.lock().unwrap();
        let mut saved = plan.clone();
        saved.proposal_id = Some(ProposalId::new(format!("pp_{}", records.len() + 1)).unwrap());
        let summary = ProposalSummary {
            proposal_id: saved.proposal_id.clone().unwrap(),
            kind: ProposalSummaryKind,
            status: ProposalSummaryStatus::SimulationOnly,
            created_at: Timestamp::new("2026-10-10T00:00:00Z").unwrap(),
            source_volume_id: source.clone(),
            satisfied: saved.satisfied,
            shortfall_bytes: saved.shortfall_bytes,
        };
        records.push(ProposalDetail {
            summary,
            inputs_digest: digest.clone(),
            plan: saved.clone(),
        });
        Ok(saved)
    }
    fn list(&self) -> EngineResult<ProposalList> {
        Ok(ProposalList {
            proposals: BoundedVec::new(
                self.0
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|p| p.summary.clone())
                    .collect(),
            )
            .unwrap(),
        })
    }
    fn get(&self, request: &ProposalRefRequest) -> EngineResult<ProposalDetail> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.summary.proposal_id == request.proposal_id)
            .cloned()
            .ok_or_else(|| EngineError::NotFound {
                message: "proposal not found".into(),
            })
    }
}

#[test]
fn placement_engine_model_candidates_and_saved_proposals_roundtrip() {
    let engine = engine();
    let input = input();
    let model = engine.placement_model(&input, deadline()).unwrap();
    assert_eq!(model.volumes.len(), 2);
    assert_eq!(model.volumes[0].pressure, TierVolumePressure::Pressure);
    assert_eq!(model.volumes[1].pressure, TierVolumePressure::Ok);
    let candidates_request = from_value(json!({"source_volume_id":"vo_source","basis":"root_children","max_groups":1,"min_bytes":"201"})).unwrap();
    assert!(engine
        .placement_candidates(&input, &candidates_request, deadline())
        .unwrap()
        .groups
        .is_empty());
    let store = MemoryStore::default();
    let mut req = request();
    let unsaved = engine
        .placement_simulate(&input, &req, deadline(), Some(&store))
        .unwrap();
    assert!(unsaved.proposal_id.is_none());
    assert!(engine.proposals_list(&store).unwrap().proposals.is_empty());
    req.save = true;
    let saved = engine
        .placement_simulate(&input, &req, deadline(), Some(&store))
        .unwrap();
    assert_eq!(engine.proposals_list(&store).unwrap().proposals.len(), 1);
    let detail = engine
        .proposals_get(
            &store,
            &ProposalRefRequest {
                proposal_id: saved.proposal_id.clone().unwrap(),
            },
        )
        .unwrap();
    assert_eq!(detail.plan, saved);
    assert_eq!(detail.summary.status, ProposalSummaryStatus::SimulationOnly);
    assert_eq!(detail.inputs_digest.as_str().len(), 71);
    assert!(matches!(
        engine.proposals_get(
            &store,
            &ProposalRefRequest {
                proposal_id: ProposalId::new("pp_missing").unwrap()
            }
        ),
        Err(EngineError::NotFound { .. })
    ));
    assert!(matches!(
        engine.placement_simulate(&input, &req, deadline(), None),
        Err(EngineError::Unavailable {
            component: Component::Placement
        })
    ));
    assert!(matches!(
        engine.placement_simulate(&input, &req, Instant::now(), Some(&store)),
        Err(EngineError::DeadlineExceeded { .. })
    ));
    assert_eq!(engine.proposals_list(&store).unwrap().proposals.len(), 1);
}

#[test]
fn placement_engine_rejected_is_byte_equal_to_core_and_node_budget_remains_synchronous() {
    let engine = engine();
    let input = input();
    let mut req = request();
    req.overrides = BoundedVec::new(vec![]).unwrap();
    req.node_budget = Int::new(0).unwrap();
    let built = build_scenario(&input, &req, deadline()).unwrap();
    let core = plan_v2(&from_value(built.scenario).unwrap(), 0).unwrap();
    let wire = engine
        .placement_simulate(&input, &req, deadline(), None)
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&wire.rejected).unwrap(),
        serde_json::to_vec(&core.rejected).unwrap()
    );
    assert_eq!(wire.search.node_budget.get(), 0);
    assert!(!serde_json::to_value(wire).unwrap()["filesystem_changed"]
        .as_bool()
        .unwrap());
}

#[test]
fn placement_tiers_prefer_declaration_then_hints_then_unknown() {
    let mut v = volume("vo_source", 100, 0, 9).volume;
    assert_eq!(tier_info(&v).tier.unwrap().get(), 9);
    v.tier.declared_tier = None;
    for (bus, seek, tier) in [
        (DeviceHintBusType::Nvme, Some(false), Some(1)),
        (DeviceHintBusType::Sata, Some(false), Some(2)),
        (DeviceHintBusType::Sata, Some(true), Some(5)),
        (DeviceHintBusType::Usb, Some(true), Some(7)),
        (DeviceHintBusType::Usb, None, None),
    ] {
        v.device.bus_type = bus;
        v.device.seek_penalty = seek;
        assert_eq!(tier_info(&v).tier.map(|t| t.get()), tier);
    }
}

#[test]
fn placement_tier_view_supports_64_volumes_while_planner_is_bounded_to_32() {
    let mut input = input();
    input.snapshot.volumes = (0..64)
        .map(|n| volume(&format!("vo_{n}"), 100, 50, 1))
        .collect();
    assert_eq!(
        engine()
            .placement_model(&input, deadline())
            .unwrap()
            .volumes
            .len(),
        64
    );
    let mut req = request();
    req.source_volume_id = VolumeId::new("vo_0").unwrap();
    assert!(matches!(
        build_scenario(&input, &req, deadline()),
        Err(EngineError::ResourceBudget { .. })
    ));
}

#[test]
fn placement_missing_allocation_and_unparseable_link_allocation_never_verify_or_panic() {
    let mut input = input();
    let file = &mut input.files.get_mut("cg_one").unwrap()[0];
    file.allocated_bytes = None;
    file.link_observation.as_mut().unwrap().allocation = "unknown".into();
    let built = build_scenario(&input, &request(), deadline()).unwrap();
    assert_eq!(built.candidates.groups[0].estimated_relief_bytes, None);
}

#[test]
fn placement_refuses_duplicate_names_unsafe_integer_and_rounding_overflow() {
    let mut input = input();
    input.files.get_mut("cg_one").unwrap()[1].node_id = NodeId::new("nd_a").unwrap();
    assert!(matches!(
        build_scenario(&input, &request(), deadline()),
        Err(EngineError::InvalidRequest { .. })
    ));
    let mut input = super::tests::input();
    input.snapshot.volumes[0].volume.capacity_bytes = Some(Bytes(MAX_SAFE_INTEGER + 1));
    assert!(matches!(
        build_scenario(&input, &request(), deadline()),
        Err(EngineError::InvalidRequest { .. })
    ));
    let mut input = super::tests::input();
    input.snapshot.volumes[1].cluster_bytes = Some(u64::MAX);
    assert!(matches!(
        build_scenario(&input, &request(), deadline()),
        Err(EngineError::InvalidRequest { .. })
    ));
}

#[test]
fn placement_volume_binding_and_refs_legacy_ids_do_not_verify_relief() {
    for mode in 0..3 {
        let mut input = input();
        match mode {
            0 => input.snapshot.volumes[0].volume_serial = None,
            1 => input.snapshot.volumes[0].volume_serial = Some("2".into()),
            _ => {
                input.snapshot.volumes[0].volume.filesystem = Some(Text::new("ReFS").unwrap());
                for file in input.files.get_mut("cg_one").unwrap() {
                    let object = file.object.as_mut().unwrap();
                    object.quality = IdentityQuality::Legacy64 {
                        reason: "fixture".into(),
                    };
                    file.link_observation.as_mut().unwrap().object = object.clone();
                }
            }
        }
        let built = build_scenario(&input, &request(), deadline()).unwrap();
        assert_eq!(built.candidates.groups[0].estimated_relief_bytes, None);
    }
}

#[test]
fn placement_explicit_heat_whatifs_and_null_overrides_do_not_change_physics() {
    let mut input = input();
    input.snapshot.subtrees[0].modified_age_days = Some(40);
    let mut req = request();
    req.overrides = BoundedVec::new(vec![]).unwrap();
    let default = build_scenario(&input, &req, deadline()).unwrap();
    assert!(default.scenario["groups"][0].get("heat").is_none());
    req.heat_policy = PlacementSimulateRequestHeatPolicy::MtimeProxyWhatif;
    let proxy = build_scenario(&input, &req, deadline()).unwrap();
    assert_eq!(proxy.scenario["groups"][0]["heat"], 0.1);
    assert!(proxy
        .assumptions
        .iter()
        .any(|s| s.as_str().contains("not access heat")));
    assert_eq!(proxy.scenario["volumes"], default.scenario["volumes"]);
    req.overrides = from_value(json!([{"group_id":"cg_one","heat":null,"pinned":null}])).unwrap();
    let cleared = build_scenario(&input, &req, deadline()).unwrap();
    assert!(cleared.scenario["groups"][0].get("heat").is_none());
    assert!(cleared.scenario["groups"][0].get("pinned").is_none());
    assert_eq!(cleared.scenario["groups"][0]["source_bytes"], 100);
    req.heat_policy = PlacementSimulateRequestHeatPolicy::AssumedOnly;
    req.overrides = BoundedVec::new(vec![]).unwrap();
    assert!(
        build_scenario(&input, &req, deadline()).unwrap().scenario["groups"][0]
            .get("heat")
            .is_none()
    );
}

#[test]
fn placement_partial_allocation_uses_logical_fallback_and_rejects_whatif() {
    let mut input = input();
    input.snapshot.subtrees[0].totals.allocated_bytes = Some(Bytes(100));
    input.snapshot.subtrees[0].totals.allocation_unknown_files = Count::new(1).unwrap();
    input.files.get_mut("cg_one").unwrap()[0].allocated_bytes = None;
    let mut req = request();
    req.relief_policy = PlacementSimulateRequestReliefPolicy::EntryAllocationWhatif;
    let built = build_scenario(&input, &req, deadline()).unwrap();
    assert_eq!(
        built.candidates.groups[0].estimate_basis,
        CandidateGroupEstimateBasis::LogicalFallback
    );
    assert_eq!(built.candidates.groups[0].source_bytes, Bytes(200));
    assert_eq!(
        built.pre_rejected[0].reason,
        PlacementPlanPreRejectedReason::ReliefUnknown
    );
}

#[cfg(windows)]
#[test]
fn placement_native_link_pass_feeds_verified_relief_on_disposable_hardlinks() {
    use loomward_windows::{identity, linkcount};
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::write(&a, [1u8; 100]).unwrap();
    std::fs::hard_link(&a, &b).unwrap();
    let expected = identity::observe(&a).unwrap().object;
    let report = linkcount::observe(
        &[
            linkcount::Entry {
                path: a,
                expected: Some(expected.clone()),
                attributes: 0,
            },
            linkcount::Entry {
                path: b,
                expected: Some(expected.clone()),
                attributes: 0,
            },
        ],
        deadline(),
    );
    assert_eq!(report.stop, None);
    let files: Vec<_> = report
        .outcomes
        .into_iter()
        .enumerate()
        .map(|(i, outcome)| {
            let linkcount::Outcome::Matched(o) = outcome else {
                panic!("native observation unknown")
            };
            FileEntry {
                node_id: NodeId::new(format!("nd_native_{i}")).unwrap(),
                logical_bytes: 100,
                allocated_bytes: Some(o.allocation.parse().unwrap()),
                object: Some(expected.clone()),
                link_observation: Some(*o),
            }
        })
        .collect();
    let allocation = files[0].allocated_bytes.unwrap();
    assert!(allocation > 0);
    let mut input = input();
    input.snapshot.volumes[0] = volume("vo_source", allocation, 0, 1);
    input.snapshot.volumes[0].volume_serial = Some(expected.volume_serial);
    input.snapshot.subtrees[0].totals.allocated_bytes = Some(Bytes(allocation * 2));
    input.snapshot.subtrees[0].totals.unique_allocated_bytes = Some(Bytes(allocation));
    input.files.insert("cg_one".into(), files);
    let mut req = request();
    req.target_free_bytes = Bytes(allocation);
    let plan = engine()
        .placement_simulate(&input, &req, deadline(), None)
        .unwrap();
    assert!(plan.satisfied);
    assert_eq!(plan.proposals[0].source_bytes_relieved, Bytes(allocation));
}

/// Commits through `MemoryStore`, then holds the call past the deadline.
struct SlowStore(MemoryStore);

impl ProposalStore for SlowStore {
    fn save(
        &self,
        source: &VolumeId,
        digest: &Digest,
        plan: &PlacementPlan,
        deadline: Instant,
    ) -> EngineResult<PlacementPlan> {
        let saved = self.0.save(source, digest, plan, deadline)?;
        std::thread::sleep(
            deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(20),
        );
        Ok(saved)
    }
    fn list(&self) -> EngineResult<ProposalList> {
        self.0.list()
    }
    fn get(&self, request: &ProposalRefRequest) -> EngineResult<ProposalDetail> {
        self.0.get(request)
    }
}

#[test]
fn placement_save_committed_at_the_deadline_returns_the_saved_proposal() {
    let engine = engine();
    let input = input();
    let store = SlowStore(MemoryStore::default());
    let mut req = request();
    req.save = true;
    let saved = engine
        .placement_simulate(
            &input,
            &req,
            Instant::now() + Duration::from_secs(2),
            Some(&store),
        )
        .expect("a committed save is reported, not a deadline error");
    assert!(saved.proposal_id.is_some());
    assert_eq!(engine.proposals_list(&store).unwrap().proposals.len(), 1);
}

#[test]
fn placement_zero_relief_group_is_excluded_and_rest_still_plans() {
    let mut inp = input();
    let mut zero = subtree("zero");
    zero.totals = from_value(json!({"files": 2, "dirs": 1, "logical_bytes": "200",
        "allocated_bytes": "0", "allocation_unknown_files": 0, "skipped": 0,
        "failed": 0, "complete": true, "stream_coverage": "default_stream_only",
        "unique_objects": 1, "unique_allocated_bytes": "0", "multi_link_entries": 2
    }))
    .unwrap();
    inp.snapshot.subtrees.push(zero);
    let mut zfiles = vec![file("z_a"), file("z_b")];
    for f in &mut zfiles {
        f.object.as_mut().unwrap().file_id = [7; 16];
        f.link_observation.as_mut().unwrap().object = f.object.clone().unwrap();
        f.allocated_bytes = Some(0);
        f.link_observation.as_mut().unwrap().allocation = "0".into();
    }
    inp.files.insert("cg_zero".into(), zfiles);
    let req = request();
    let built = build_scenario(&inp, &req, deadline()).unwrap();
    assert_eq!(built.scenario["groups"].as_array().unwrap().len(), 1);
    assert_eq!(built.scenario["groups"][0]["id"], "cg_one");
    assert!(
        built
            .assumptions
            .iter()
            .any(|s| s.as_str().contains("zero relief") && s.as_str().contains('1')),
        "assumptions: {:?}",
        built.assumptions
    );
    let plan = engine()
        .placement_simulate(&inp, &req, deadline(), None)
        .unwrap();
    assert_eq!(plan.proposals.len(), 1);
    assert_eq!(plan.proposals[0].group_id.as_str(), "cg_one");
}

#[test]
fn placement_unknown_cluster_uses_4kib_lower_bound_with_disclosure() {
    let mut all_unknown = input();
    all_unknown.snapshot.volumes[0].cluster_bytes = None;
    all_unknown.snapshot.volumes[1].cluster_bytes = None;
    let built = build_scenario(&all_unknown, &request(), deadline()).unwrap();
    assert_eq!(built.candidates.groups[0].destination_bytes, Bytes(8192));
    assert!(
        built
            .assumptions
            .iter()
            .any(|s| s.as_str().contains("4 KiB") && s.as_str().contains('1')),
        "assumptions: {:?}",
        built.assumptions
    );
    let mut mixed = input();
    let mut extra = volume("vo_extra", 1000, 1000, 2);
    extra.cluster_bytes = None;
    mixed.snapshot.volumes.push(extra);
    let built = build_scenario(&mixed, &request(), deadline()).unwrap();
    assert_eq!(built.candidates.groups[0].destination_bytes, Bytes(8192));
    assert!(
        built
            .assumptions
            .iter()
            .any(|s| s.as_str().contains("4 KiB")),
        "assumptions: {:?}",
        built.assumptions
    );
}

#[test]
fn placement_children_of_pre_rejected_parent_are_disclosed_not_planned() {
    let mut inp = input();
    inp.snapshot.subtrees.clear();
    inp.files.clear();
    let mut parent = subtree("aaa_parent");
    parent.totals = from_value(json!({"files": 2, "dirs": 1, "logical_bytes": "400",
        "allocated_bytes": "400", "allocation_unknown_files": 0, "skipped": 0,
        "failed": 0, "complete": false, "stream_coverage": "default_stream_only",
        "unique_objects": 1, "unique_allocated_bytes": "100", "multi_link_entries": 2
    }))
    .unwrap();
    let parent_node = parent.node_id.clone();
    let parent_group = parent.group_id.clone();
    inp.snapshot.subtrees.push(parent);
    for (id, fid) in [("zzz_child1", [21u8; 16]), ("zzz_child2", [22u8; 16])] {
        let mut child = subtree(id);
        child.ancestors = vec![parent_node.clone()];
        child.root_child = false;
        inp.snapshot.subtrees.push(child.clone());
        let mut files = vec![file(&format!("{id}_a")), file(&format!("{id}_b"))];
        for f in &mut files {
            f.object.as_mut().unwrap().file_id = fid;
            f.link_observation.as_mut().unwrap().object = f.object.clone().unwrap();
        }
        inp.files.insert(child.group_id.as_str().into(), files);
    }
    let mut pfiles = vec![file("p_a"), file("p_b")];
    for f in &mut pfiles {
        f.object.as_mut().unwrap().file_id = [20; 16];
        f.link_observation.as_mut().unwrap().object = f.object.clone().unwrap();
    }
    inp.files.insert(parent_group.as_str().into(), pfiles);
    let mut req = request();
    req.overrides = BoundedVec::new(vec![]).unwrap();
    let built = build_scenario(&inp, &req, deadline()).unwrap();
    assert!(built.pre_rejected.iter().any(|r| r.group_id == parent_group));
    assert!(
        built
            .assumptions
            .iter()
            .any(|s| s.as_str().contains("2") && s.as_str().contains("ancestor was pre-rejected")),
        "assumptions: {:?}",
        built.assumptions
    );
}

#[test]
fn placement_unusable_volume_numbers_stay_unknown_in_tier_view() {
    for mode in 0..3 {
        let mut inp = input();
        let mut bad = volume("vo_bad", 100, 50, 2);
        match mode {
            0 => bad.volume.free_bytes = Some(Bytes(200)),
            1 => bad.reserve_bytes = Bytes(200),
            _ => bad.cluster_bytes = Some(0),
        }
        inp.snapshot.volumes.push(bad);
        let model = engine().placement_model(&inp, deadline()).unwrap();
        let view = model
            .volumes
            .iter()
            .find(|v| v.volume_id.as_str() == "vo_bad")
            .unwrap();
        assert_eq!(view.pressure, TierVolumePressure::Unknown, "mode {mode}");
    }
}
