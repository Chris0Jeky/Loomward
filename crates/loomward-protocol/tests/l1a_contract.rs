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
        catalog_rev: Some(Generation(8812)),
        state_rev: None,
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
    #[derive(Debug, serde::Deserialize, serde::Serialize)]
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

fn event(seq: u32) -> EventEnvelope {
    EventEnvelope {
        protocol: Protocol,
        epoch: StreamEpoch::new("e_k3J9x2Qa").unwrap(),
        seq: Count::from(seq),
        event: EventName::StreamHello,
        at: Timestamp::new("2026-10-09T12:00:00.250Z").unwrap(),
        catalog_rev: None,
        state_rev: None,
        data: Default::default(),
    }
}

#[test]
fn event_envelope_needs_epoch_and_both_revisions() {
    let full = json!({"protocol":"loomward/3","epoch":"e_k3J9x2Qa","seq":4211,"event":"scan.progress","at":"2026-10-09T12:00:00Z","catalog_rev":"8813","state_rev":null,"data":{}});
    let env: EventEnvelope = serde_json::from_value(full.clone()).unwrap();
    assert_eq!(env.event, EventName::ScanProgress);
    assert_eq!(env.catalog_rev, Some(Generation(8813)));
    assert_eq!(serde_json::to_value(&env).unwrap(), full);
    for missing in ["epoch", "catalog_rev", "state_rev"] {
        let mut v = full.clone();
        v.as_object_mut().unwrap().remove(missing);
        assert!(
            serde_json::from_value::<EventEnvelope>(v).is_err(),
            "{missing}"
        );
    }
    let mut unknown = full;
    unknown["event"] = json!("scan.finished");
    assert!(serde_json::from_value::<EventEnvelope>(unknown).is_err());
}

#[test]
fn complete_envelope_checks_pair_payload_with_command_and_event() {
    // A job.state event whose data is not a JobResult fails the complete check.
    let mut e = event(1);
    e.event = EventName::JobState;
    e.data = json!({"root_id": null, "generation": null, "scope": "all", "node_ids": []})
        .as_object()
        .unwrap()
        .clone();
    assert!(e.validate().is_err());
    // A request that carries a precondition where none exists is invalid_request.
    let mut req = RequestEnvelope::new(
        RequestId::new("r_1").unwrap(),
        Command::JobsList.name(),
        json!({"limit": 5}).as_object().unwrap().clone(),
    );
    assert_eq!(req.validate().unwrap(), Command::JobsList);
    req.expected_state_rev = Some(Generation(3));
    assert_eq!(req.validate().unwrap_err().code, ErrorCode::InvalidRequest);
    req.command = Command::VolumesDeclareTier.name();
    req.payload = json!({"volume_id": "vo_c1", "tier": 2})
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(req.validate().unwrap(), Command::VolumesDeclareTier);
    req.command = CommandName::new("files.erase").unwrap();
    assert_eq!(req.validate().unwrap_err().code, ErrorCode::UnknownCommand);
}

/// A toy stream: shows the trait is object safe and that `close` is idempotent and final.
struct ScriptedStream {
    epoch: String,
    queue: std::collections::VecDeque<EventEnvelope>,
    closed: bool,
}

impl EventStream for ScriptedStream {
    fn epoch(&self) -> &str {
        &self.epoch
    }
    fn recv_timeout(&mut self, _timeout: Duration) -> RecvOutcome {
        if self.closed {
            return RecvOutcome::Closed(CloseReason::ClosedByAdapter);
        }
        self.queue
            .pop_front()
            .map_or(RecvOutcome::Timeout, RecvOutcome::Event)
    }
    fn close(&mut self) {
        self.closed = true;
    }
}

#[test]
fn event_stream_trait_yields_then_closes() {
    let mut s: Box<dyn EventStream> = Box::new(ScriptedStream {
        epoch: "e_k3J9x2Qa".into(),
        queue: [event(1)].into(),
        closed: false,
    });
    assert_eq!(s.epoch(), "e_k3J9x2Qa");
    assert!(
        matches!(s.recv_timeout(Duration::from_millis(1)), RecvOutcome::Event(e) if e.seq.get() == 1)
    );
    assert_eq!(
        s.recv_timeout(Duration::from_millis(1)),
        RecvOutcome::Timeout
    );
    s.close();
    s.close();
    assert_eq!(
        s.recv_timeout(Duration::from_millis(1)),
        RecvOutcome::Closed(CloseReason::ClosedByAdapter)
    );
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
    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
        Box::new(ScriptedStream {
            epoch: resume.map_or_else(|| "e_fresh000".into(), |(e, _)| e),
            queue: Default::default(),
            closed: false,
        })
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
    let stream = svc.subscribe(Some(("e_k3J9x2Qa".into(), 7)));
    assert_eq!(stream.epoch(), "e_k3J9x2Qa");
}

#[test]
fn strict_decode_refuses_positional_arrays_and_accepts_integral_floats() {
    // serde would read this as a tuple struct; the contract only knows objects.
    assert_eq!(
        RequestEnvelope::from_slice(br#"["loomward/3","r_1","a.b",{}]"#)
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    let ok = RequestEnvelope::from_slice(
        br#"{"protocol":"loomward/3","request_id":"r_1","command":"tree.slice","payload":{"anchor":{"kind":"atlas"},"depth":2,"max_nodes":100,"min_share":0,"basis":"logical","include_files":false}}"#,
    )
    .unwrap();
    // `1` where the DTO holds an f64 re-serialises as 1.0: equal by value, so accepted.
    let frac: Fraction = serde_json::from_value(json!(1)).unwrap();
    assert_eq!(decode_exact::<Fraction>(json!(1)).unwrap(), frac);
    assert_eq!(ok.command.as_str(), "tree.slice");
    assert_eq!(
        RequestEnvelope::from_slice(b"{not json").unwrap_err().code,
        ErrorCode::InvalidRequest
    );
}

fn request_for(command: Command, payload: Value) -> RequestEnvelope {
    RequestEnvelope::new(
        RequestId::new("r_1").unwrap(),
        command.name(),
        payload.as_object().unwrap().clone(),
    )
}

#[test]
fn expected_generation_is_only_for_single_root_anchored_commands() {
    let root_slice = json!({"anchor":{"kind":"root","root_id":"rt_a"},"depth":2,"max_nodes":100,"min_share":0.0,"basis":"logical","include_files":false});
    let atlas_slice = json!({"anchor":{"kind":"atlas"},"depth":2,"max_nodes":100,"min_share":0.0,"basis":"logical","include_files":false});
    let ok_cases = [
        (Command::TreeSlice, root_slice.clone()),
        (
            Command::TreeChildren,
            json!({"node_id":"nd_1","sort":"size_desc","basis":"logical","limit":10,"cursor":null}),
        ),
        (Command::TreePath, json!({"node_id":"nd_1"})),
        (Command::NodeInspect, json!({"node_id":"nd_1"})),
        (
            Command::StatsBreakdown,
            json!({"node_id":"nd_1","by":"extension","basis":"logical","limit":5}),
        ),
    ];
    for (command, payload) in ok_cases {
        let mut req = request_for(command, payload);
        req.expected_generation = Some(Generation(12));
        assert_eq!(req.validate().unwrap(), command, "{}", command.as_str());
    }
    // An atlas slice spans every root, so there is no single generation to expect.
    let mut atlas = request_for(Command::TreeSlice, atlas_slice);
    assert!(atlas.validate().is_ok());
    atlas.expected_generation = Some(Generation(12));
    assert_eq!(
        atlas.validate().unwrap_err().code,
        ErrorCode::InvalidRequest
    );
    // Every other command refuses it.
    for command in Command::ALL {
        if matches!(
            command,
            Command::TreeSlice
                | Command::TreeChildren
                | Command::TreePath
                | Command::NodeInspect
                | Command::StatsBreakdown
        ) {
            continue;
        }
        let mut req = request_for(*command, json!({}));
        req.expected_generation = Some(Generation(12));
        // Payload validity is checked first; use the command's own example payload.
        let path = format!(
            "{}/../../contracts/v3/examples/commands/{}.request.json",
            env!("CARGO_MANIFEST_DIR"),
            command.as_str()
        );
        req.payload = serde_json::from_str::<Value>(&std::fs::read_to_string(path).unwrap())
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        assert!(req.validate().is_err(), "{}", command.as_str());
        req.expected_generation = None;
        assert!(
            req.validate().is_ok(),
            "{} without the precondition",
            command.as_str()
        );
    }
}

#[test]
fn from_slice_is_the_complete_entry_point() {
    // A scan.start payload sent as tree.slice: structurally an object, wrong for the command.
    let wrong = br#"{"protocol":"loomward/3","request_id":"r_1","command":"tree.slice","payload":{"root_id":"rt_a","mode":"full"}}"#;
    assert_eq!(
        RequestEnvelope::from_slice(wrong).unwrap_err().code,
        ErrorCode::InvalidRequest
    );
    let unknown =
        br#"{"protocol":"loomward/3","request_id":"r_1","command":"files.erase","payload":{}}"#;
    assert_eq!(
        RequestEnvelope::from_slice(unknown).unwrap_err().code,
        ErrorCode::UnknownCommand
    );
    let precondition = br#"{"protocol":"loomward/3","request_id":"r_1","command":"scan.start","payload":{"root_id":"rt_a","mode":"full"},"expected_state_rev":"3"}"#;
    assert_eq!(
        RequestEnvelope::from_slice(precondition).unwrap_err().code,
        ErrorCode::InvalidRequest
    );
    let good = br#"{"protocol":"loomward/3","request_id":"r_1","command":"scan.start","payload":{"root_id":"rt_a","mode":"full"}}"#;
    assert_eq!(
        RequestEnvelope::from_slice(good).unwrap().command.as_str(),
        "scan.start"
    );
}

#[test]
fn integers_above_two_to_the_53_are_not_conflated() {
    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    struct Wide {
        x: f64,
    }
    // 2^53 round-trips exactly through f64; 2^53 + 1 does not and must be refused.
    assert!(decode_exact::<Wide>(json!({"x": 9007199254740992u64})).is_ok());
    assert!(decode_exact::<Wide>(json!({"x": 9007199254740993u64})).is_err());
    assert!(decode_exact::<Wide>(json!({"x": 1})).is_ok());
    assert!(decode_exact::<Wide>(json!({"x": 0.5})).is_ok());
}
