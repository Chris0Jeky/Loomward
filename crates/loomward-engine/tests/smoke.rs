//! Smoke tests for implemented infrastructure and explicitly unavailable future lanes.

use loomward_engine::events::EventResume;
use loomward_engine::jobs::JobSpec;
use loomward_engine::{Component, Engine, EngineConfig, EngineError, EngineResult};
use loomward_protocol::{DatasetClass, Detail, ErrorCode, JobId, JobKind};
use serde_json::{from_value, json};
use std::path::PathBuf;

fn engine(dataset_class: DatasetClass) -> Engine {
    Engine::open(EngineConfig::new(
        PathBuf::from("unused-state-dir"),
        dataset_class,
    ))
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
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join("absent").join("state");
    let file = temp.path().join("file");
    std::fs::write(&file, b"synthetic sentinel").unwrap();
    for path in [absent, file.clone(), temp.path().to_owned()] {
        for dataset in [DatasetClass::Synthetic, DatasetClass::Personal] {
            let config = EngineConfig::new(path.clone(), dataset);
            let e = Engine::open(config.clone()).unwrap();
            assert_eq!(e.config().dataset_class, dataset);
            assert_eq!(e.config().state_dir, path);
            assert_eq!(e.config(), &config);
            drop(e);
        }
    }
    assert!(!temp.path().join("absent").exists());
    assert_eq!(std::fs::read(file).unwrap(), b"synthetic sentinel");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn structured_error_details_survive_wire_serialization() {
    let detail = |key: &str, value: &str| {
        Some(
            Detail::new(std::collections::BTreeMap::from([(
                key.into(),
                json!(value),
            )]))
            .unwrap(),
        )
    };
    let table = [
        (
            EngineError::Busy {
                message: "scan active".into(),
                detail: detail("job_id", "jb_fixture"),
            },
            ErrorCode::Busy,
            "job_id",
            "jb_fixture",
            true,
        ),
        (
            EngineError::StaleGeneration {
                message: "state moved".into(),
                detail: detail("reason", "state_rev_changed"),
            },
            ErrorCode::StaleGeneration,
            "reason",
            "state_rev_changed",
            true,
        ),
        (
            EngineError::Internal {
                message: "overflow".into(),
                detail: detail("reason", "byte_overflow"),
            },
            ErrorCode::InternalError,
            "reason",
            "byte_overflow",
            false,
        ),
    ];
    for (error, code, key, value, retryable) in table {
        let body = error.to_body();
        assert_eq!(body.code, code);
        assert_eq!(body.retryable, retryable);
        assert_eq!(
            serde_json::to_value(&body).unwrap()["detail"],
            json!({key: value})
        );
        assert_eq!(loomward_protocol::ErrorBody::from(error), body);
    }
}

#[test]
fn implemented_infrastructure_and_unavailable_future_lanes() {
    let e = engine(DatasetClass::Synthetic);
    let job_id = JobId::new("jb_smoke").unwrap();
    assert!(e.job_submit(JobSpec::new(JobKind::Refit, None)).is_err());
    assert!(e.job_cancel(&job_id).is_err());
    assert!(e.job_status(&job_id).is_err());
    assert!(e
        .job_list(&from_value(json!({"limit":10})).unwrap())
        .unwrap()
        .jobs
        .is_empty());
    assert!(e.scan_cancel(&job_id).is_err());
    assert!(e.subscribe_events(None).is_ok());
    assert!(e
        .subscribe_events(Some(EventResume {
            epoch: "epoch-smoke".into(),
            last_seq: 7
        }))
        .is_ok());

    // L11b telemetry is live; detailed lifecycle/schema/Windows checks live in telemetry.rs.
    assert!(matches!(
        e.telemetry_snapshot(&from_value(json!({ "channels": ["system"] })).unwrap()),
        Err(EngineError::PartialCoverage { .. })
    ));
    assert!(matches!(
        e.telemetry_lease(
            &from_value(json!({
                "subscription_id": null, "channels": ["system"], "interval_ms": 1000
            }))
            .unwrap()
        ),
        Err(EngineError::PermissionDenied { .. })
    ));

    // Placement is implemented: its snapshot/store entry points are exercised in placement::tests.

    // learning
    assert_unavailable(
        e.learning_feedback(
            &from_value(json!({
                "node_id": "nd_smoke", "label": null, "retract": true,
                "client_event_id": "evt_smoke_1"
            }))
            .unwrap(),
            1,
            &json!({}),
        ),
        Component::Learning,
    );
    assert_unavailable(
        e.learning_queue(
            &from_value(json!({ "root_id": null, "limit": 10, "cursor": null })).unwrap(),
            vec![],
            (0, 0),
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

    assert!(e.budgets_get().is_ok());
    assert!(e
        .budgets_set(&from_value(json!({ "pool": "learning", "max_workers": 2 })).unwrap())
        .is_ok());
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
            EngineError::StaleGeneration {
                message: m("x"),
                detail: None,
            },
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
        (
            EngineError::Busy {
                message: m("x"),
                detail: None,
            },
            ErrorCode::Busy,
            true,
        ),
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
            EngineError::Internal {
                message: m("x"),
                detail: None,
            },
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
