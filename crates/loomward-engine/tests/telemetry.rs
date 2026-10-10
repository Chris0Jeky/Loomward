//! Read-only telemetry through the engine boundary (no process effects).
use loomward_engine::{Engine, EngineConfig, EngineError};
use loomward_protocol::*;
use serde_json::{from_value, json, to_value, Value};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

fn engine() -> Engine {
    Engine::open(EngineConfig::new(
        PathBuf::from("unused-telemetry-state"),
        DatasetClass::Personal,
    ))
    .unwrap()
}

fn lease(engine: &Engine, id: Option<SubscriptionId>, interval: u32) -> TelemetrySubscription {
    engine.telemetry_lease(&from_value(json!({
        "subscription_id": id, "channels": ["system", "engine", "processes", "gpu", "disks"],
        "interval_ms": interval
    })).unwrap()).unwrap()
}

fn latest(engine: &Engine) -> TelemetrySample {
    let request =
        from_value(json!({"channels": ["system", "engine", "processes", "gpu", "disks"]})).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match engine.telemetry_snapshot(&request) {
            Ok(sample) => return sample,
            Err(EngineError::PartialCoverage { .. }) if Instant::now() < until => {
                thread::sleep(Duration::from_millis(10))
            }
            result => return result.unwrap(),
        }
    }
}

fn validate(def: &str, value: &Value) {
    let mut schema: Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/view-service.schema.json"
    ))
    .unwrap();
    schema.as_object_mut().unwrap().remove("anyOf");
    schema["$ref"] = json!(format!("#/$defs/{def}"));
    let validator = jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap();
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{def}: {errors:?}");
}

#[test]
fn leases_share_renew_release_and_latest_is_not_an_on_demand_scan() {
    let e = engine();
    assert!(matches!(
        e.telemetry_snapshot(&from_value(json!({"channels":["engine"]})).unwrap()),
        Err(EngineError::PartialCoverage { .. })
    ));
    let first = lease(&e, None, 1000);
    let other = lease(&e, None, 10000);
    assert_ne!(first.subscription_id, other.subscription_id);
    let sample = latest(&e);
    assert!(sample.elapsed_ms.is_none());
    assert_eq!(sample.engine.as_ref().unwrap().pools.len(), 1);
    let renewed = lease(&e, Some(first.subscription_id.clone()), 2000);
    assert_eq!(first.subscription_id, renewed.subscription_id);
    assert_eq!(renewed.interval_ms, TelemetryInterval::TwoSeconds);
    for subscription in [&first, &other] {
        let released = e
            .telemetry_release(&SubscriptionRefRequest {
                subscription_id: subscription.subscription_id.clone(),
            })
            .unwrap();
        assert!(released.stopped);
        assert!(matches!(
            e.telemetry_release(&SubscriptionRefRequest {
                subscription_id: subscription.subscription_id.clone()
            }),
            Err(EngineError::NotFound { .. })
        ));
    }
    let stopped = latest(&e);
    thread::sleep(Duration::from_millis(1100));
    assert_eq!(stopped.sample_seq, latest(&e).sample_seq);
}

#[test]
fn produced_samples_lists_and_explanations_validate_against_the_schema() {
    let e = engine();
    let subscription = lease(&e, None, 1000);
    let sample = latest(&e);
    validate("TelemetrySample", &to_value(&sample).unwrap());
    assert_eq!(sample, from_value(to_value(&sample).unwrap()).unwrap());
    validate("TelemetrySubscription", &to_value(&subscription).unwrap());
    let health = e.telemetry_health().unwrap();
    validate("Health/properties/engine", &to_value(&health).unwrap());
    assert_eq!(health, from_value(to_value(&health).unwrap()).unwrap());
    let list = e.processes_list(&from_value(json!({"sort":"private_desc","limit":1})).unwrap());
    if cfg!(not(windows)) {
        assert!(matches!(list, Err(EngineError::PartialCoverage { .. })));
        assert!(sample.system.is_none());
        assert!(sample.processes.is_none());
        return;
    }
    let list = list.unwrap();
    validate("ProcessList", &to_value(&list).unwrap());
    assert!(list.rows.len() <= 1);
    for row in &list.rows {
        let explanation = e
            .processes_explain(&ProcessRefRequest {
                process_ref: row.process_ref.clone(),
            })
            .unwrap();
        validate("ProcessExplanation", &to_value(&explanation).unwrap());
        assert!(explanation.available_actions.is_empty());
        assert!(explanation
            .facts
            .iter()
            .any(|f| f.code.as_str() == "gpu_memory_unknown"));
    }
    for event in e.telemetry_events().unwrap() {
        validate("TelemetrySampleEvent", &to_value(&event).unwrap());
    }
    e.telemetry_release(&SubscriptionRefRequest {
        subscription_id: subscription.subscription_id,
    })
    .unwrap();
}

#[test]
fn intervals_channels_and_limits_are_bounded_before_engine_calls() {
    for interval in [0, 999, 1001, 60000] {
        assert!(from_value::<TelemetrySubscribeRequest>(
            json!({"subscription_id":null,"channels":["engine"],"interval_ms":interval})
        )
        .is_err());
    }
    for interval in [1000, 2000, 5000, 10000] {
        assert!(from_value::<TelemetrySubscribeRequest>(
            json!({"subscription_id":null,"channels":["engine"],"interval_ms":interval})
        )
        .is_ok());
    }
    for channels in [json!([]), json!(["engine", "engine"])] {
        assert!(from_value::<TelemetrySnapshotRequest>(json!({"channels": channels})).is_err());
    }
    assert!(from_value::<ProcessListRequest>(json!({"sort":"cpu_desc","limit":201})).is_err());
}

#[cfg(windows)]
#[test]
fn windows_engine_fields_have_structural_truths_without_assuming_token_rights() {
    let e = engine();
    let subscription = lease(&e, None, 1000);
    let first = latest(&e);
    let system = first.system.unwrap();
    assert!(system.cpu.logical_cpus.get() > 0);
    assert!(system.memory.total_bytes.unwrap().get() > 0);
    assert!(system.memory.available_bytes.unwrap() <= system.memory.total_bytes.unwrap());
    let own = first.engine.unwrap();
    assert!(own.private_commit_bytes.unwrap().get() > 0);
    assert!(own.cpu_fraction.is_none());
    let until = Instant::now() + Duration::from_secs(10);
    let second = loop {
        let sample = latest(&e);
        if sample.sample_seq > first.sample_seq {
            break sample;
        }
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(20));
    };
    assert!(second.elapsed_ms.unwrap().get() >= 1000);
    assert!((0.0..=1.0).contains(&second.engine.unwrap().cpu_fraction.unwrap().get()));
    let list = e
        .processes_list(&from_value(json!({"sort":"name_asc","limit":200})).unwrap())
        .unwrap();
    for row in list.rows.iter().filter(|p| p.pid.get() == 4) {
        assert_ne!(row.working_set_bytes, Some(Bytes(0)));
        if row.working_set_bytes.is_none() {
            assert_ne!(row.access, ProcessRowAccess::Full);
        }
    }
    e.telemetry_release(&SubscriptionRefRequest {
        subscription_id: subscription.subscription_id,
    })
    .unwrap();
}
