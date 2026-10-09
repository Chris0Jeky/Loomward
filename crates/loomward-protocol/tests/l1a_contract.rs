//! L1a: the envelopes, the closed command/event lists and the primitive bounds.

use loomward_protocol::*;
use serde_json::{json, Value};
use std::time::Duration;

fn commands_json() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/v3/commands.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("commands.json"))
        .expect("valid JSON")
}

#[test]
fn command_table_equals_commands_json() {
    let doc = commands_json();
    let commands = doc["commands"].as_object().unwrap();
    assert_eq!(commands.len(), 45);
    assert_eq!(Command::ALL.len(), commands.len());
    for c in Command::ALL {
        let row = &commands[c.as_str()];
        assert_eq!(row["request"], c.request_def(), "{}", c.as_str());
        assert_eq!(row["result"], c.result_def(), "{}", c.as_str());
        assert_eq!(row["wave"], c.wave(), "{}", c.as_str());
        assert_eq!(row["effects"], "none", "{}", c.as_str());
        let adapters: Vec<_> = row["adapters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let ours: Vec<_> = c
            .adapters()
            .iter()
            .map(|a| serde_json::to_value(a).unwrap())
            .collect();
        assert_eq!(
            adapters,
            ours.iter().map(|a| a.as_str().unwrap()).collect::<Vec<_>>()
        );
        let events: Vec<_> = row
            .get("events")
            .and_then(Value::as_array)
            .map(|e| e.iter().map(|e| e.as_str().unwrap()).collect())
            .unwrap_or_default();
        assert_eq!(
            events,
            c.events().iter().map(|e| e.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn event_table_equals_commands_json() {
    let doc = commands_json();
    let events = doc["events"].as_object().unwrap();
    assert_eq!(events.len(), 10);
    assert_eq!(EventName::ALL.len(), events.len());
    for e in EventName::ALL {
        assert_eq!(events[e.as_str()]["data"], e.data_def(), "{}", e.as_str());
        assert_eq!(serde_json::to_value(e).unwrap(), e.as_str());
    }
}

#[test]
fn no_command_reads_like_an_effect() {
    for c in Command::ALL {
        for bad in command::FORBIDDEN_NAME_PARTS {
            assert!(!c.as_str().contains(bad), "{} contains {bad}", c.as_str());
        }
        assert_eq!(Command::from_name(c.as_str()), Some(*c));
        assert_eq!(c.name().as_str(), c.as_str());
    }
    assert_eq!(Command::from_name("files.delete"), None);
}

#[test]
fn http_lacks_the_native_picker() {
    assert!(!Command::RootsRequestGrant.available_on(Adapter::Http));
    assert!(Command::RootsRequestGrant.available_on(Adapter::Tauri));
}

#[test]
fn bytes_are_decimal_strings_only() {
    assert_eq!(
        serde_json::from_value::<Bytes>(json!("0")).unwrap(),
        Bytes(0)
    );
    assert_eq!(
        serde_json::to_value(Bytes(u64::MAX)).unwrap(),
        json!("18446744073709551615")
    );
    for bad in [
        json!(5),
        json!(5.5),
        json!("05"),
        json!("-1"),
        json!(""),
        json!("1e3"),
        json!("18446744073709551616"),
        json!("99999999999999999999"),
        json!(null),
    ] {
        assert!(
            serde_json::from_value::<Bytes>(bad.clone()).is_err(),
            "{bad}"
        );
    }
    // Unknown stays null, never zero.
    assert_eq!(
        serde_json::to_value(Option::<Bytes>::None).unwrap(),
        Value::Null
    );
}

#[test]
fn counts_stop_at_two_to_the_53() {
    assert!(Count::new(MAX_SAFE_INTEGER).is_ok());
    assert!(Count::new(MAX_SAFE_INTEGER + 1).is_err());
    assert!(serde_json::from_value::<Count>(json!(-1)).is_err());
    assert!(serde_json::from_value::<Count>(json!("5")).is_err());
}

#[test]
fn timestamps_are_utc_and_real() {
    for ok in ["2026-10-09T12:00:00Z", "2024-02-29T00:00:00.123456789Z"] {
        assert!(Timestamp::new(ok).is_ok(), "{ok}");
    }
    for bad in [
        "2026-10-09T12:00:00",
        "2026-10-09T12:00:00+01:00",
        "2026-13-09T12:00:00Z",
        "2025-02-29T00:00:00Z",
        "2026-10-09 12:00:00Z",
        "2026-10-09T24:00:00Z",
        "2026-10-09T12:00:00.Z",
        "2026-10-09t12:00:00z",
    ] {
        assert!(Timestamp::new(bad).is_err(), "{bad}");
    }
}

#[test]
fn ids_need_their_prefix() {
    assert!(RootId::new("rt_abc-1").is_ok());
    assert!(RootId::new("vo_abc").is_err());
    assert!(RootId::new("rt_").is_err());
    assert!(NodeId::new("nd_a b").is_err());
    assert!(RequestId::new("r_000017").is_ok());
    assert!(CommandName::new("tree.slice").is_ok());
    assert!(CommandName::new("tree").is_err());
    assert!(CommandName::new("Tree.slice").is_err());
}

#[test]
fn bounded_collections_enforce_their_limits() {
    type Ids = BoundedVec<u8, 1, 3>;
    assert!(serde_json::from_value::<Ids>(json!([1, 2, 3])).is_ok());
    assert!(serde_json::from_value::<Ids>(json!([])).is_err());
    assert!(serde_json::from_value::<Ids>(json!([1, 2, 3, 4])).is_err());
    type Uniq = UniqueVec<u8, 0, 3>;
    assert!(serde_json::from_value::<Uniq>(json!([1, 2])).is_ok());
    assert!(serde_json::from_value::<Uniq>(json!([1, 1])).is_err());
}

#[test]
fn effect_constants_refuse_true() {
    assert!(serde_json::from_value::<ConstFalse>(json!(false)).is_ok());
    assert!(serde_json::from_value::<ConstFalse>(json!(true)).is_err());
    assert!(serde_json::from_value::<Protocol>(json!("loomward/2")).is_err());
}

fn sample_meta() -> ResponseMeta {
    ResponseMeta {
        served_at: Timestamp::new("2026-10-09T12:00:00Z").unwrap(),
        elapsed_ms: Count::from(14),
        dataset_class: DatasetClass::Synthetic,
        budget_hit: false,
    }
}

#[test]
fn request_envelope_fails_closed_on_unknown_fields() {
    let good = json!({"protocol":"loomward/3","request_id":"r_1","command":"tree.slice","payload":{},"deadline_ms":2000});
    let env: RequestEnvelope = serde_json::from_value(good.clone()).unwrap();
    assert_eq!(serde_json::to_value(&env).unwrap(), good);
    let mut extra = good.clone();
    extra["path"] = json!("C:\\x");
    assert!(serde_json::from_value::<RequestEnvelope>(extra).is_err());
    let mut late = good;
    late["deadline_ms"] = json!(60001);
    assert!(serde_json::from_value::<RequestEnvelope>(late).is_err());
    let mut null_deadline =
        json!({"protocol":"loomward/3","request_id":"r_1","command":"a.b","payload":{}});
    null_deadline["deadline_ms"] = Value::Null;
    assert!(serde_json::from_value::<RequestEnvelope>(null_deadline).is_err());
}

#[test]
fn responses_round_trip_and_report_real_errors() {
    let ok = ResponseEnvelope::ok(
        RequestId::new("r_1").unwrap(),
        &json!({"jobs": []}),
        sample_meta(),
    );
    let v = serde_json::to_value(&ok).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(serde_json::from_value::<ResponseEnvelope>(v).unwrap(), ok);

    let err = ResponseEnvelope::error(
        None,
        ErrorBody::new(ErrorCode::UnknownCommand, "no such command", false),
        None,
    );
    let v = serde_json::to_value(&err).unwrap();
    assert_eq!(v["request_id"], Value::Null);
    assert!(v.get("meta").is_none());
    assert_eq!(serde_json::from_value::<ResponseEnvelope>(v).unwrap(), err);

    // A body that claims ok:true but is an error shape fails with the field error.
    let msg = serde_json::from_value::<ResponseEnvelope>(
        json!({"protocol":"loomward/3","request_id":"r_1","ok":true,"error":{}}),
    )
    .unwrap_err()
    .to_string();
    assert!(
        msg.contains("unknown field `error`") || msg.contains("result"),
        "{msg}"
    );

    // request_id is required even when null.
    assert!(serde_json::from_value::<ResponseEnvelope>(json!({"protocol":"loomward/3","ok":false,"error":{"code":"internal_error","message":"x","retryable":false,"detail":null}})).is_err());
}

#[test]
fn non_object_result_becomes_internal_error() {
    let r = ResponseEnvelope::ok(
        RequestId::new("r_1").unwrap(),
        &json!([1, 2]),
        sample_meta(),
    );
    match r {
        ResponseEnvelope::Err(e) => assert_eq!(e.error.code, ErrorCode::InternalError),
        ResponseEnvelope::Ok(_) => panic!("array is not a result object"),
    }
}

#[test]
fn payload_decoding_reports_invalid_request() {
    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct One {
        #[allow(dead_code)]
        a: u8,
    }
    let mut env = RequestEnvelope::new(
        RequestId::new("r_1").unwrap(),
        Command::JobsGet.name(),
        Default::default(),
    );
    env.payload.insert("b".into(), json!(1));
    let err = env.decode_payload::<One>().unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRequest);
    assert!(!err.retryable);
}

#[test]
fn event_envelope_serialises_sse_fields() {
    let env: EventEnvelope = serde_json::from_value(json!({"protocol":"loomward/3","seq":4211,"event":"scan.progress","at":"2026-10-09T12:00:00Z","data":{}})).unwrap();
    assert_eq!(env.event, EventName::ScanProgress);
    assert!(serde_json::from_value::<EventEnvelope>(json!({"protocol":"loomward/3","seq":1,"event":"scan.finished","at":"2026-10-09T12:00:00Z","data":{}})).is_err());
}

#[test]
fn subscription_is_bounded() {
    let (tx, sub) = EventSubscription::bounded(2);
    let event = |seq| EventEnvelope {
        protocol: Protocol,
        seq: Count::from(seq),
        event: EventName::StreamHello,
        at: Timestamp::new("2026-10-09T12:00:00Z").unwrap(),
        data: Default::default(),
    };
    tx.try_send(event(1)).unwrap();
    tx.try_send(event(2)).unwrap();
    assert!(
        tx.try_send(event(3)).is_err(),
        "a full queue must refuse, not grow"
    );
    assert_eq!(
        sub.recv_timeout(Duration::from_millis(10))
            .unwrap()
            .seq
            .get(),
        1
    );
    drop(tx);
    assert_eq!(sub.try_recv().unwrap().seq.get(), 2);
    assert!(sub.recv_timeout(Duration::from_millis(10)).is_err());
}

struct Fake;
impl ViewService for Fake {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        if Command::from_name(&request.command).is_none() {
            return ResponseEnvelope::error(
                Some(request.request_id),
                ErrorBody::new(ErrorCode::UnknownCommand, "unknown", false),
                None,
            );
        }
        ResponseEnvelope::ok(
            request.request_id,
            &json!({"adapter": ctx.adapter}),
            sample_meta(),
        )
    }
    fn subscribe(&self, _last_seq: Option<u64>) -> EventSubscription {
        EventSubscription::bounded(1).1
    }
}

#[test]
fn trait_is_object_safe_and_usable() {
    let svc: std::sync::Arc<dyn ViewService> = std::sync::Arc::new(Fake);
    let req = RequestEnvelope::new(
        RequestId::new("r_1").unwrap(),
        Command::SessionHello.name(),
        Default::default(),
    );
    let resp = svc.call(req, &CallContext::http());
    assert!(resp.is_ok());
    let _ = svc.subscribe(None);
}
