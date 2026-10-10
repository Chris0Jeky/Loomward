//! Skeleton smoke test: every public entry point answers `Unavailable` for its own component.
//! A lane that lands an entry point must change its line here; a lane that forgets one stays
//! visible because the skeleton answer is still asserted.

use loomward_engine::events::EventResume;
use loomward_engine::jobs::JobSpec;
use loomward_engine::scan::GrantedRoot;
use loomward_engine::{Component, Engine, EngineConfig, EngineError, EngineResult};
use loomward_protocol::{DatasetClass, ErrorCode, JobId, JobKind, RootId};
use serde_json::{from_value, json};
use std::path::PathBuf;

fn engine(dataset_class: DatasetClass) -> Engine {
    Engine::open(EngineConfig {
        state_dir: PathBuf::from("unused-state-dir"),
        dataset_class,
    })
    .expect("the skeleton opens without touching the filesystem")
}

/// Asserts `result` is `Unavailable { component }` (without needing `T: Debug`).
fn assert_unavailable<T>(result: EngineResult<T>, component: Component) {
    match result {
        Err(EngineError::Unavailable { component: got }) => assert_eq!(got, component),
        Err(other) => panic!("expected Unavailable({component:?}), got {other}"),
        Ok(_) => panic!("expected Unavailable({component:?}), got Ok"),
    }
}

#[test]
fn open_touches_nothing_and_keeps_config() {
    let e = engine(DatasetClass::Synthetic);
    assert_eq!(e.config().dataset_class, DatasetClass::Synthetic);
    assert_eq!(e.config().state_dir, PathBuf::from("unused-state-dir"));
    assert!(!PathBuf::from("unused-state-dir").exists());
}

#[test]
fn every_entry_point_is_unavailable() {
    let e = engine(DatasetClass::Synthetic);
    let job_id = JobId::new("jb_smoke").unwrap();
    let root = GrantedRoot::new(
        RootId::new("rt_smoke").unwrap(),
        PathBuf::from("unused-root"),
        DatasetClass::Synthetic,
    );

    // jobs
    assert_unavailable(
        e.job_submit(JobSpec::new(JobKind::Scan, Some(root.root_id.clone()))),
        Component::Jobs,
    );
    assert_unavailable(e.job_cancel(&job_id), Component::Jobs);
    assert_unavailable(e.job_status(&job_id), Component::Jobs);
    assert_unavailable(
        e.job_list(&from_value(json!({ "limit": 10 })).unwrap()),
        Component::Jobs,
    );

    // scan
    assert_unavailable(e.scan_start(&root, None), Component::Scan);
    assert_unavailable(e.scan_refresh(&root, None), Component::Scan);
    assert_unavailable(e.scan_cancel(&job_id), Component::Scan);

    // events
    assert_unavailable(e.subscribe_events(None), Component::Events);
    assert_unavailable(
        e.subscribe_events(Some(EventResume {
            epoch: "epoch-smoke".to_string(),
            last_seq: 7,
        })),
        Component::Events,
    );

    // telemetry
    assert_unavailable(
        e.telemetry_lease(
            &from_value(json!({
                "subscription_id": null, "channels": ["system"], "interval_ms": 1000
            }))
            .unwrap(),
        ),
        Component::Telemetry,
    );
    assert_unavailable(
        e.telemetry_release(&from_value(json!({ "subscription_id": "sb_smoke" })).unwrap()),
        Component::Telemetry,
    );
    assert_unavailable(
        e.telemetry_snapshot(&from_value(json!({ "channels": ["system"] })).unwrap()),
        Component::Telemetry,
    );
    assert_unavailable(
        e.processes_list(&from_value(json!({ "sort": "cpu_desc", "limit": 10 })).unwrap()),
        Component::Telemetry,
    );
    assert_unavailable(
        e.processes_explain(&from_value(json!({ "process_ref": "pc_smoke" })).unwrap()),
        Component::Telemetry,
    );

    // placement
    assert_unavailable(e.placement_model(), Component::Placement);
    assert_unavailable(
        e.placement_candidates(
            &from_value(json!({
                "source_volume_id": "vo_smoke", "basis": "root_children",
                "max_groups": 5, "min_bytes": "0"
            }))
            .unwrap(),
        ),
        Component::Placement,
    );
    assert_unavailable(
        e.placement_simulate(
            &from_value(json!({
                "source_volume_id": "vo_smoke", "target_free_bytes": "0",
                "max_transfer_bytes": "0", "heat_policy": "unknown_is_ineligible",
                "relief_policy": "verified_only", "candidate_basis": "root_children",
                "max_groups": 5, "overrides": [], "node_budget": 1000, "save": false
            }))
            .unwrap(),
        ),
        Component::Placement,
    );
    assert_unavailable(e.proposals_list(), Component::Placement);
    assert_unavailable(
        e.proposals_get(&from_value(json!({ "proposal_id": "pp_smoke" })).unwrap()),
        Component::Placement,
    );

    // learning
    assert_unavailable(
        e.learning_feedback(
            &from_value(json!({
                "node_id": "nd_smoke", "label": null, "retract": true,
                "client_event_id": "evt_smoke_1"
            }))
            .unwrap(),
        ),
        Component::Learning,
    );
    assert_unavailable(
        e.learning_queue(
            &from_value(json!({ "root_id": null, "limit": 10, "cursor": null })).unwrap(),
        ),
        Component::Learning,
    );
    assert_unavailable(e.learning_refit(), Component::Learning);

    // teacher
    assert_unavailable(
        e.teacher_preview(
            &from_value(json!({
                "node_ids": ["nd_smoke"], "recipient": "codex_cli_gpt_6_1_sol"
            }))
            .unwrap(),
        ),
        Component::Teacher,
    );
    assert_unavailable(
        e.teacher_run(&from_value(json!({ "grant_id": "gr_smoke" })).unwrap()),
        Component::Teacher,
    );

    // budgets
    assert_unavailable(e.budgets_get(), Component::Budgets);
    assert_unavailable(
        e.budgets_set(&from_value(json!({ "pool": "learning", "max_workers": 2 })).unwrap()),
        Component::Budgets,
    );
}

#[test]
fn personal_engine_refuses_teacher_run_before_anything_else() {
    let e = engine(DatasetClass::Personal);
    let err = e
        .teacher_run(&from_value(json!({ "grant_id": "gr_smoke" })).unwrap())
        .unwrap_err();
    assert_eq!(
        err,
        EngineError::PermissionDenied {
            message: "confinement_not_enforced".to_string()
        }
    );
    // Preview sends nothing, so it is not gated here.
    assert_unavailable(
        e.teacher_preview(
            &from_value(json!({
                "node_ids": ["nd_smoke"], "recipient": "codex_cli_gpt_6_1_sol"
            }))
            .unwrap(),
        ),
        Component::Teacher,
    );
}

#[test]
fn errors_map_to_protocol_codes() {
    let m = |message: &str| message.to_string();
    let table = [
        (
            EngineError::unavailable(Component::Scan),
            ErrorCode::CapabilityUnavailable,
            false,
        ),
        (
            EngineError::InvalidRequest { message: m("x") },
            ErrorCode::InvalidRequest,
            false,
        ),
        (
            EngineError::NotFound { message: m("x") },
            ErrorCode::NotFound,
            false,
        ),
        (
            EngineError::PermissionDenied { message: m("x") },
            ErrorCode::PermissionDenied,
            false,
        ),
        (
            EngineError::StaleGeneration { message: m("x") },
            ErrorCode::StaleGeneration,
            true,
        ),
        (
            EngineError::DeviceOffline { message: m("x") },
            ErrorCode::DeviceOffline,
            true,
        ),
        (
            EngineError::PartialCoverage { message: m("x") },
            ErrorCode::PartialCoverage,
            true,
        ),
        (
            EngineError::ResourceBudget { message: m("x") },
            ErrorCode::ResourceBudget,
            true,
        ),
        (EngineError::Busy { message: m("x") }, ErrorCode::Busy, true),
        (
            EngineError::Cancelled { message: m("x") },
            ErrorCode::Cancelled,
            false,
        ),
        (
            EngineError::DeadlineExceeded { message: m("x") },
            ErrorCode::DeadlineExceeded,
            true,
        ),
        (
            EngineError::Internal { message: m("x") },
            ErrorCode::InternalError,
            false,
        ),
    ];
    for (err, code, retryable) in table {
        let body = err.to_body();
        assert_eq!(body.code, code, "{err}");
        assert_eq!(body.retryable, retryable, "{err}");
        assert!(!err.to_string().is_empty());
    }
}
