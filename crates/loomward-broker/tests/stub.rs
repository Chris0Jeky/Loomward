//! The broker binary against an in-process OpenAI-compatible stub on 127.0.0.1: the provider
//! contract, the wire shape, refusals that send nothing, timeouts, bounds and a three-round turn
//! with a fresh broker per round. No external network.

use loomward_ask::testkit::{call, calls, contract, text, Upstream};
use loomward_ask::turn::ask;
use loomward_ask::*;
use loomward_protocol::{CallContext, EventStream, RequestEnvelope, ResponseEnvelope, ViewService};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

fn exe() -> PathBuf {
    env!("CARGO_BIN_EXE_loomward-broker").into()
}

/// One HTTP request as the stub received it.
#[derive(Debug, Clone)]
struct Seen {
    head: String,
    body: Value,
    /// The body bytes exactly as received.
    raw: Vec<u8>,
}

/// Serves one scripted raw response per connection, then stops accepting.
struct Stub {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    thread: Option<JoinHandle<()>>,
}

fn read_request(s: &mut TcpStream) -> Seen {
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = s.read(&mut buf).unwrap();
        raw.extend_from_slice(&buf[..n]);
        if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8(raw[..p].to_vec()).unwrap();
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .unwrap()
                .parse()
                .unwrap();
            if raw.len() >= p + 4 + len {
                let raw = raw[p + 4..p + 4 + len].to_vec();
                let body = serde_json::from_slice(&raw).unwrap();
                return Seen { head, body, raw };
            }
        }
        assert!(n > 0, "request ended early");
    }
}

impl Stub {
    /// `None` in the script holds the connection open without answering (a stall).
    fn start(script: Vec<Option<Vec<u8>>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let thread = std::thread::spawn(move || {
            for reply in script {
                let (mut s, _) = listener.accept().unwrap();
                log.lock().unwrap().push(read_request(&mut s));
                match reply {
                    Some(bytes) => {
                        let _ = s.write_all(&bytes);
                    }
                    None => std::thread::sleep(Duration::from_millis(1500)),
                }
            }
        });
        Self {
            port,
            seen,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    fn seen(&mut self) -> Vec<Seen> {
        self.thread.take().unwrap().join().unwrap();
        self.seen.lock().unwrap().clone()
    }
}

fn response(status: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn completion(reply: &ChatReply) -> Vec<u8> {
    let message =
        json!({"role": "assistant", "content": reply.content, "tool_calls": reply.tool_calls});
    response(
        "200 OK",
        &json!({"id": "x", "object": "chat.completion", "choices": [{"index": 0, "message": message}]})
            .to_string(),
    )
}

/// A port nothing listens on.
fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn provider(url: &str, model: &str) -> BrokerProvider {
    let profile = ProviderProfile {
        base_url: url.into(),
        model: model.into(),
    };
    open(&profile, exe(), None).unwrap()
}

fn round() -> ChatRequest {
    ChatRequest {
        messages: vec![Message::User {
            content: "hello".into(),
        }],
        tools: vec![],
    }
}

const T: Duration = Duration::from_secs(20);

#[test]
fn the_broker_meets_the_provider_contract_over_loopback() {
    contract(&|script: Vec<Upstream>| {
        let [only] = <[Upstream; 1]>::try_from(script).unwrap();
        let url = match only {
            Upstream::Reply(r) => {
                // Leaked deliberately: the stub thread ends after its one connection.
                let stub = Stub::start(vec![Some(completion(&r))]);
                let url = stub.url();
                std::mem::forget(stub);
                url
            }
            Upstream::Down => format!("http://127.0.0.1:{}/v1", closed_port()),
            Upstream::Fail(_) => unreachable!("fake only"),
        };
        Box::new(provider(&url, "synthetic-model"))
    });
}

#[test]
fn the_wire_is_one_plain_chat_completions_post_with_no_credential() {
    let mut stub = Stub::start(vec![Some(completion(&text("ok")))]);
    let mut req = round();
    req.tools = vec![json!({"type": "function", "function": {"name": "find"}})];
    assert_eq!(
        provider(&stub.url(), "qwen3:4b").complete(&req, T).unwrap(),
        text("ok")
    );
    let seen = stub.seen();
    assert_eq!(seen.len(), 1);
    let head = seen[0].head.to_ascii_lowercase();
    assert!(
        head.starts_with("post /v1/chat/completions http/1.1\r\n"),
        "{head}"
    );
    assert!(head.contains(&format!("host: 127.0.0.1:{}", stub.port)));
    assert!(!head.contains("authorization"));
    assert_eq!(
        seen[0].body,
        json!({"model": "qwen3:4b", "messages": [{"role": "user", "content": "hello"}],
               "tools": req.tools, "stream": false})
    );
}

#[test]
fn remote_models_and_non_loopback_endpoints_send_nothing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "http://127.0.0.1:{}/v1",
        listener.local_addr().unwrap().port()
    );
    // Through the gate: a loopback Ollama `:cloud` model is the cloud class.
    let cloud = ProviderProfile {
        base_url: url.clone(),
        model: "big-model:cloud".into(),
    };
    assert_eq!(
        open(&cloud, exe(), None).unwrap_err().kind,
        ErrorKind::ConsentRequired
    );
    // Around the gate: the broker re-checks every call itself.
    for (endpoint, model) in [
        (url.as_str(), "big-model:cloud"),
        ("http://192.0.2.1:80/v1", "m"),
        ("https://127.0.0.1:443/v1", "m"),
        (url.as_str(), ""),
    ] {
        let e = loomward_broker::call(&BrokerRequest {
            endpoint: endpoint.into(),
            model: model.into(),
            limits: Limits::default(),
            deadline_ms: 5_000,
            request: round(),
        })
        .unwrap_err();
        assert_eq!(
            (e.kind, e.dispatched),
            (ErrorKind::NotLocal, false),
            "{endpoint} {model}"
        );
    }
    assert!(listener.accept().is_err(), "nothing connected");
}

#[test]
fn redirects_errors_bounds_and_stalls_are_honest_failures() {
    let big = format!("{{\"pad\":\"{}\"}}", "x".repeat(MAX_REPLY_BYTES));
    let mut stub = Stub::start(vec![
        Some(response("307 Temporary Redirect", "")),
        Some(response("500 Internal Server Error", "{}")),
        Some(response("200 OK", &big)),
        Some(response("200 OK", "{\"choices\":[]}")),
        None,
    ]);
    let mut p = provider(&stub.url(), "m");
    let kinds: Vec<(ErrorKind, bool)> = (0..4)
        .map(|_| p.complete(&round(), T).unwrap_err())
        .map(|e| (e.kind, e.dispatched))
        .collect();
    assert_eq!(
        kinds,
        [
            (ErrorKind::HttpStatus, true),
            (ErrorKind::HttpStatus, true),
            (ErrorKind::Oversize, true),
            (ErrorKind::InvalidReply, true)
        ]
    );
    p.limits.first_byte_ms = 300;
    let e = p.complete(&round(), T).unwrap_err();
    assert_eq!((e.kind, e.dispatched), (ErrorKind::Timeout, true), "{e:?}");
    // The redirect was not followed: five requests, five connections.
    assert_eq!(stub.seen().len(), 5);
}

/// Answers `search.query` with nothing, as the client principal it was called with.
struct EmptyView(Mutex<Vec<String>>);

impl ViewService for EmptyView {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        self.0
            .lock()
            .unwrap()
            .push(format!("{:?} {}", ctx.principal, request.command.as_ref()));
        serde_json::from_value(json!({
            "protocol": "loomward/3", "request_id": "r1", "ok": true,
            "result": {"hits": [], "next_cursor": null, "roots": [], "generation": "g1"},
            "meta": {"served_at": "2026-10-17T00:00:00Z", "elapsed_ms": 0, "dataset_class": "synthetic",
                     "budget_hit": false, "catalog_rev": null, "state_rev": null}
        }))
        .unwrap()
    }
    fn subscribe(&self, _: Option<(String, u64)>) -> Box<dyn EventStream> {
        unimplemented!()
    }
}

#[test]
fn a_three_round_turn_spawns_a_fresh_broker_for_each_round() {
    let find = |id: &str| completion(&calls(vec![call(id, "find", r#"{"text":"report"}"#)]));
    let mut stub = Stub::start(vec![
        Some(find("c1")),
        Some(find("c2")),
        Some(completion(&text("Nothing matched."))),
    ]);
    let profile = ProviderProfile {
        base_url: stub.url(),
        model: "m".into(),
    };
    let p = open(&profile, exe(), None).unwrap();
    let view = EmptyView(Mutex::default());
    let mut log = Vec::new();
    let answer = ask(&view, &p, &profile, "find my report", &mut log).unwrap();
    assert_eq!(
        (answer.text.as_str(), answer.rounds),
        ("Nothing matched.", 3)
    );
    let seen = stub.seen();
    assert_eq!(seen.len(), 3, "one connection per round");
    assert_eq!(seen[2].body["messages"].as_array().unwrap().len(), 6);
    assert_eq!(
        *view.0.lock().unwrap(),
        [r#"Client("ask:local") search.query"#; 2]
    );
    // The record's digest and length are of the exact body bytes the stub received (#560).
    for (rec, got) in log.iter().zip(&seen) {
        assert_eq!(rec.bytes, got.raw.len());
        assert_eq!(rec.sha256, format!("sha256:{:x}", Sha256::digest(&got.raw)));
    }
    assert!(log
        .iter()
        .all(|r| r.dispatched && r.outcome == "ok" && r.principal == "ask:local"));
    // An outage mid-turn ends it; nothing else is tried.
    let p = provider(&format!("http://127.0.0.1:{}", closed_port()), "m");
    let mut log = Vec::new();
    let e = ask(&view, &p, &profile, "again", &mut log).unwrap_err();
    assert_eq!((e.kind, e.dispatched), (ErrorKind::Unreachable, false));
    assert_eq!(log.len(), 1);
}
