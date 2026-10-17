//! LW-118: the Ask commands over the scripted fake provider. Owner-only, desktop-only profiles,
//! no contact without a profile and a grant, citations from the ledger, the tool log, the round
//! records, revocation from the next call and the next turn. Synthetic names only.

use super::*;
use crate::clients::tests::{open, root, ALPHA};
use crate::tests::temp_root;
use loomward_ask::testkit::{call, calls, text, ScriptedProvider, Upstream};
use loomward_ask::{ChatReply, ChatRequest, Message};
use loomward_protocol::{Command, NativeDialogs, RootId};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct NoDialogs;
impl NativeDialogs for NoDialogs {
    fn pick_folder(&self) -> Option<PathBuf> {
        panic!("an ask command opened the folder picker")
    }
    fn confirm_disclosure(
        &self,
        _: &loomward_protocol::DisclosureSummary,
    ) -> Result<bool, ErrorBody> {
        panic!("an ask command opened the disclosure dialog")
    }
}

static IDS: AtomicUsize = AtomicUsize::new(0);

fn call_as(svc: &Service, ctx: &CallContext, command: &str, payload: Value) -> ResponseEnvelope {
    let id = IDS.fetch_add(1, Ordering::SeqCst);
    let req = serde_json::from_value(json!({
        "protocol": "loomward/3", "request_id": format!("ask_t{id}"), "command": command,
        "payload": payload
    }))
    .unwrap();
    svc.call(req, ctx)
}
fn owner(svc: &Service, command: &str, payload: Value) -> ResponseEnvelope {
    call_as(svc, &CallContext::http(), command, payload)
}
fn desktop(svc: &Service, command: &str, payload: Value) -> ResponseEnvelope {
    call_as(
        svc,
        &CallContext::tauri(Arc::new(NoDialogs)),
        command,
        payload,
    )
}
fn ok(r: ResponseEnvelope) -> Value {
    match r {
        ResponseEnvelope::Ok(o) => Value::Object(o.result),
        ResponseEnvelope::Err(e) => panic!("{:?}", e.error),
    }
}
fn err(r: ResponseEnvelope) -> (ErrorCode, Option<String>) {
    match r {
        ResponseEnvelope::Ok(o) => panic!("expected an error, got {:?}", o.result),
        ResponseEnvelope::Err(e) => (
            e.error.code,
            e.error
                .detail
                .as_ref()
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
    }
}

/// A provider the test keeps a handle on: every turn's rounds go to the same script.
#[derive(Clone)]
struct Shared(Arc<ScriptedProvider>, Arc<AtomicUsize>);
impl Provider for Shared {
    fn complete(&self, req: &ChatRequest, d: Duration) -> Result<ChatReply, ProviderError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.complete(req, d)
    }
}

fn fake(svc: &Service, script: Vec<Upstream>) -> Shared {
    let p = Shared(
        Arc::new(ScriptedProvider::new(script)),
        Arc::new(AtomicUsize::new(0)),
    );
    let q = p.clone();
    *svc.inner.ask.factory.lock().unwrap() = Arc::new(move |_| Ok(Box::new(q.clone())));
    p
}

const PROFILE: &str = "http://127.0.0.1:11434/v1";

fn configured(dir: &std::path::Path) -> (Service, RootId) {
    let svc = open(dir);
    let rt = root(&svc, "Alpha", 1, Some(ALPHA));
    let s = ok(desktop(
        &svc,
        "ask.configure",
        json!({"profile": {"base_url": PROFILE, "model": "synthetic-model:3b"}}),
    ));
    assert_eq!(s["provider"], "local");
    (svc, rt)
}

fn settled(svc: &Service, turn: &Value) -> Value {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let t = ok(owner(
            svc,
            "ask.result",
            json!({"turn_id": turn["turn_id"]}),
        ));
        if t["state"] != "running" {
            return t;
        }
        assert!(Instant::now() < until, "the turn never settled");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn every_ask_command_is_the_owners_and_the_profile_is_desktop_only() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let ask: Vec<Command> = Command::ALL
        .iter()
        .copied()
        .filter(|c| c.as_str().starts_with("ask."))
        .collect();
    assert_eq!(ask.len(), 6);
    for c in &ask {
        let req = serde_json::from_str::<Value>(
            &std::fs::read_to_string(format!(
                "{}/../../contracts/v3/examples/commands/{}.request.json",
                env!("CARGO_MANIFEST_DIR"),
                c.as_str()
            ))
            .unwrap(),
        )
        .unwrap();
        // The model's own principal, or any other client, never reaches them.
        for client in [PRINCIPAL, "mcp:host"] {
            let r = call_as(&svc, &CallContext::client(client), c.as_str(), req.clone());
            // ask.configure is not even on a client's (HTTP-shaped) adapter.
            let want = if *c == Command::AskConfigure {
                ErrorCode::CapabilityUnavailable
            } else {
                ErrorCode::PermissionDenied
            };
            assert_eq!(err(r).0, want, "{}", c.as_str());
        }
    }
    let profile = json!({"profile": {"base_url": PROFILE, "model": "m"}});
    let (code, _) = err(owner(&svc, "ask.configure", profile.clone()));
    assert_eq!(code, ErrorCode::CapabilityUnavailable);
    assert_eq!(
        ok(owner(&svc, "ask.status", json!({})))["provider"],
        "not_configured"
    );
    assert_eq!(
        ok(desktop(&svc, "ask.configure", profile))["provider"],
        "local"
    );
    let s = ok(desktop(&svc, "ask.configure", json!({"profile": null})));
    assert_eq!(
        (s["provider"].as_str(), &s["profile"]),
        (Some("not_configured"), &Value::Null)
    );
}

#[test]
fn a_cloud_profile_is_refused_and_nothing_is_stored() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    for (url, model) in [
        ("https://api.example.invalid/v1", "m"),
        ("http://192.0.2.1:11434/v1", "m"),
        ("http://127.0.0.1:11434/v1", "gpt-oss:120b-cloud"),
    ] {
        let r = desktop(
            &svc,
            "ask.configure",
            json!({"profile": {"base_url": url, "model": model}}),
        );
        assert_eq!(
            err(r),
            (
                ErrorCode::CapabilityUnavailable,
                Some("ask_cloud_gated".into())
            ),
            "{url} {model}"
        );
    }
    assert_eq!(
        ok(owner(&svc, "ask.status", json!({})))["profile"],
        Value::Null
    );
}

#[test]
fn nothing_is_sent_without_a_profile_or_a_grant() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let rt = root(&svc, "Alpha", 1, Some(ALPHA));
    let p = fake(&svc, vec![Upstream::Reply(text("never"))]);
    let q = json!({"question": "where is my report?"});
    assert_eq!(
        err(owner(&svc, "ask.turn", q.clone())),
        (
            ErrorCode::CapabilityUnavailable,
            Some("ask_not_configured".into())
        )
    );
    ok(desktop(
        &svc,
        "ask.configure",
        json!({"profile": {"base_url": PROFILE, "model": "m"}}),
    ));
    assert_eq!(
        err(owner(&svc, "ask.turn", q.clone())),
        (ErrorCode::PermissionDenied, Some("ask_no_grant".into()))
    );
    // A grant whose only root was revoked sees nothing, so it is no grant.
    ok(owner(
        &svc,
        "ask.grant",
        json!({"roots": [rt], "fields": []}),
    ));
    ok(owner(
        &svc,
        "roots.revoke",
        json!({"root_id": rt, "purge_catalog": false}),
    ));
    assert_eq!(
        err(owner(&svc, "ask.turn", q)).1.as_deref(),
        Some("ask_no_grant")
    );
    assert_eq!(p.1.load(Ordering::SeqCst), 0, "no round was sent");
}

#[test]
fn a_turn_answers_with_ledger_citations_the_tool_log_and_round_records() {
    let tmp = temp_root();
    let (svc, rt) = configured(tmp.path());
    let s = ok(owner(
        &svc,
        "ask.grant",
        json!({"roots": [rt], "fields": ["path", "size"]}),
    ));
    assert_eq!(s["grant"]["roots"], json!([rt]));
    assert_eq!(s["grant"]["fields"], json!(["path", "size"]));
    assert!(s["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["read_only"] == true));
    let p = fake(
        &svc,
        vec![
            Upstream::Reply(calls(vec![
                call("c1", "find", r#"{"text":"report.pdf"}"#),
                call("c2", "delete", r#"{"handle":"h1"}"#),
            ])),
            Upstream::Reply(text(
                "h1 is your report. You could archive it; I cannot. h9 is a guess.",
            )),
        ],
    );
    let running = ok(owner(
        &svc,
        "ask.turn",
        json!({"question": "where is my report?"}),
    ));
    assert_eq!(running["state"], "running");
    let t = settled(&svc, &running);
    assert_eq!(t["state"], "completed", "{t}");
    assert_eq!(
        t["answer"],
        "h1 is your report. You could archive it; I cannot. h9 is a guess."
    );
    let cite = &t["citations"][0];
    assert_eq!(t["citations"].as_array().unwrap().len(), 1);
    assert_eq!(
        (
            &cite["handle"],
            &cite["resolved"],
            &cite["name"],
            &cite["root_id"]
        ),
        (&json!("h1"), &json!(true), &json!("report.pdf"), &json!(rt))
    );
    // The cited node is a real catalogue entry the owner can open.
    let inspect = ok(owner(
        &svc,
        "node.inspect",
        json!({"node_id": cite["node_id"]}),
    ));
    assert_eq!(inspect["row"]["name"], "report.pdf");
    assert_eq!(t["unverified"], json!(["h9"]));
    assert_eq!(t["tool_calls_total"], 2);
    assert_eq!(t["tool_calls"][0]["name"], "find");
    assert_eq!(t["tool_calls"][0]["ok"], true);
    assert_eq!(
        (
            &t["tool_calls"][1]["name"],
            &t["tool_calls"][1]["ok"],
            &t["tool_calls"][1]["detail"]
        ),
        (&json!("delete"), &json!(false), &json!("unknown tool"))
    );
    let rounds = t["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 2);
    let digest = ok(owner(&svc, "ask.status", json!({})))["profile_digest"].clone();
    let seen = p.0.seen.lock().unwrap();
    for (i, r) in rounds.iter().enumerate() {
        assert_eq!(
            (&r["round"], &r["dispatched"], &r["outcome"]),
            (&json!(i + 1), &json!(true), &json!("ok"))
        );
        assert_eq!(r["principal"], PRINCIPAL);
        assert_eq!(r["profile_digest"], digest);
        let body = loomward_ask::request_body("synthetic-model:3b", &seen[i]);
        assert_eq!(r["bytes"], body.len());
    }
    // The model saw the delete refused and no node ID.
    let Message::Tool { content, .. } = &seen[1].messages[4] else {
        panic!()
    };
    assert_eq!(content, r#"{"error":"unknown tool"}"#);
    assert!(!serde_json::to_string(&seen[1].messages)
        .unwrap()
        .contains("nd_"));
    assert_eq!(
        ok(owner(&svc, "ask.status", json!({})))["last_turn"],
        t["turn_id"]
    );
}

#[test]
fn a_revocation_holds_from_the_next_call_and_the_next_turn() {
    let tmp = temp_root();
    let (svc, rt) = configured(tmp.path());
    ok(owner(
        &svc,
        "ask.grant",
        json!({"roots": [rt], "fields": []}),
    ));
    // Revoked between round 1 and round 2: round 2's tool call reads nothing.
    let script = ScriptedProvider::new(vec![
        Upstream::Reply(calls(vec![call("c1", "find", r#"{"text":"report"}"#)])),
        Upstream::Reply(calls(vec![call("c2", "find", r#"{"text":"report"}"#)])),
        Upstream::Reply(text("done")),
    ]);
    struct RevokeAfterFirst(ScriptedProvider, Weak<Inner>, AtomicUsize);
    impl Provider for RevokeAfterFirst {
        fn complete(&self, r: &ChatRequest, d: Duration) -> Result<ChatReply, ProviderError> {
            if self.2.fetch_add(1, Ordering::SeqCst) == 1 {
                self.1.upgrade().unwrap().revoke_client(PRINCIPAL).unwrap();
            }
            self.0.complete(r, d)
        }
    }
    let slot = Arc::new(Mutex::new(Some(script)));
    let weak = Arc::downgrade(&svc.inner);
    let seen = Arc::new(Mutex::new(Vec::<ChatRequest>::new()));
    let keep = seen.clone();
    *svc.inner.ask.factory.lock().unwrap() = Arc::new(move |_| {
        struct Recording(RevokeAfterFirst, Arc<Mutex<Vec<ChatRequest>>>);
        impl Provider for Recording {
            fn complete(&self, r: &ChatRequest, d: Duration) -> Result<ChatReply, ProviderError> {
                self.1.lock().unwrap().push(r.clone());
                self.0.complete(r, d)
            }
        }
        let script = slot.lock().unwrap().take().expect("one turn");
        Ok(Box::new(Recording(
            RevokeAfterFirst(script, weak.clone(), AtomicUsize::new(0)),
            keep.clone(),
        )))
    });
    let t = settled(&svc, &ok(owner(&svc, "ask.turn", json!({"question": "q"}))));
    assert_eq!(t["state"], "completed");
    assert_eq!(t["tool_calls"][0]["ok"], true);
    assert_eq!(
        (&t["tool_calls"][1]["ok"], &t["tool_calls"][1]["detail"]),
        (&json!(false), &json!("permission_denied"))
    );
    let seen = seen.lock().unwrap();
    let Message::Tool { content, .. } = seen[2].messages.last().unwrap() else {
        panic!()
    };
    assert_eq!(content, r#"{"error":"permission_denied"}"#);
    drop(seen);
    // The next turn is refused before anything is sent.
    assert_eq!(
        ok(owner(&svc, "ask.status", json!({})))["grant"],
        Value::Null
    );
    assert_eq!(
        err(owner(&svc, "ask.turn", json!({"question": "q"})))
            .1
            .as_deref(),
        Some("ask_no_grant")
    );
    // ask.revoke itself, from a fresh grant.
    ok(owner(
        &svc,
        "ask.grant",
        json!({"roots": [rt], "fields": []}),
    ));
    assert_eq!(
        ok(owner(&svc, "ask.revoke", json!({})))["grant"],
        Value::Null
    );
    assert_eq!(
        err(owner(&svc, "ask.turn", json!({"question": "q"})))
            .1
            .as_deref(),
        Some("ask_no_grant")
    );
}

#[test]
fn an_unreachable_model_fails_the_turn_honestly_and_one_turn_runs_at_a_time() {
    let tmp = temp_root();
    let (svc, rt) = configured(tmp.path());
    ok(owner(
        &svc,
        "ask.grant",
        json!({"roots": [rt], "fields": []}),
    ));
    fake(&svc, vec![Upstream::Down]);
    let t = settled(&svc, &ok(owner(&svc, "ask.turn", json!({"question": "q"}))));
    assert_eq!(t["state"], "failed");
    assert_eq!(
        (
            &t["failure"]["kind"],
            &t["failure"]["dispatched"],
            &t["answer"]
        ),
        (&json!("unreachable"), &json!(false), &Value::Null)
    );
    assert_eq!(t["rounds"][0]["outcome"], "unreachable");
    // A turn whose model is still thinking holds the slot.
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = Arc::new(Mutex::new(rx));
    *svc.inner.ask.factory.lock().unwrap() = Arc::new(move |_| {
        struct Waits(Arc<Mutex<std::sync::mpsc::Receiver<()>>>);
        impl Provider for Waits {
            fn complete(&self, _: &ChatRequest, _: Duration) -> Result<ChatReply, ProviderError> {
                let _ = self.0.lock().unwrap().recv_timeout(Duration::from_secs(20));
                Ok(text("late"))
            }
        }
        Ok(Box::new(Waits(rx.clone())))
    });
    let first = ok(owner(&svc, "ask.turn", json!({"question": "q"})));
    assert_eq!(
        err(owner(&svc, "ask.turn", json!({"question": "q2"}))).0,
        ErrorCode::Busy
    );
    tx.send(()).unwrap();
    assert_eq!(settled(&svc, &first)["answer"], "late");
    assert_eq!(
        err(owner(&svc, "ask.result", json!({"turn_id": "at_1"}))).0,
        ErrorCode::NotFound
    );
}

#[test]
fn a_failed_ask_turn_keeps_successful_and_refused_lookups() {
    let tmp = temp_root();
    let (svc, rt) = configured(tmp.path());
    ok(owner(
        &svc,
        "ask.grant",
        json!({"roots": [rt], "fields": []}),
    ));
    fake(
        &svc,
        vec![
            Upstream::Reply(calls(vec![
                call("c1", "find", r#"{"text":"report.pdf"}"#),
                call("c2", "delete", r#"{"handle":"h1"}"#),
            ])),
            Upstream::Down,
        ],
    );
    let t = settled(&svc, &ok(owner(&svc, "ask.turn", json!({"question": "q"}))));
    assert_eq!(t["state"], "failed");
    assert_eq!(t["tool_calls_total"], 2);
    assert_eq!(t["tool_calls"][0]["name"], "find");
    assert_eq!(t["tool_calls"][0]["ok"], true);
    assert_eq!(t["tool_calls"][1]["name"], "delete");
    assert_eq!(t["tool_calls"][1]["ok"], false);
}

#[test]
fn a_failed_ask_turn_reports_any_round_that_was_sent() {
    let tmp = temp_root();
    let (svc, rt) = configured(tmp.path());
    ok(owner(
        &svc,
        "ask.grant",
        json!({"roots": [rt], "fields": []}),
    ));
    fake(
        &svc,
        vec![
            Upstream::Reply(calls(vec![call("c1", "find", r#"{"text":"report.pdf"}"#)])),
            Upstream::Down,
        ],
    );
    let t = settled(&svc, &ok(owner(&svc, "ask.turn", json!({"question": "q"}))));
    assert_eq!(t["rounds"][0]["dispatched"], true);
    assert_eq!(t["rounds"][1]["dispatched"], false);
    assert_eq!(t["failure"]["dispatched"], true);
}

#[test]
fn the_contract_holds_every_handle_a_turn_can_mint() {
    // AskTurnRecord.citations is BoundedVec<_, 0, 160>: a ledger-backed citation is never cut.
    assert_eq!(loomward_ask::turn::MAX_LEDGER, 160);
    let cites: Vec<Value> = (1..=160)
        .map(|i| json!({"handle": format!("h{i}"), "node_id": "nd_x", "resolved": false, "name": null, "root_id": null}))
        .collect();
    let mut t: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/../../contracts/v3/examples/commands/ask.result.result.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    t["citations"] = json!(cites);
    let Value::Object(map) = t else { panic!() };
    Command::AskResult.validate_result(&map).unwrap();
}
