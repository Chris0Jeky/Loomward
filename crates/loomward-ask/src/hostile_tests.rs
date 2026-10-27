//! LW-145: hostile file names and metadata through every Ask tool, over a real service.
//!
//! A lab folder holds the shared corpus (`loomward_service::hostile`); the model is scripted to
//! call every tool in [`TOOLS`] on it, and invented tools and arguments besides. The suite asserts
//! what reaches the model and the owner's tool log, and that the service saw reads only and its
//! tables did not change.

use super::*;
use crate::testkit::{call, calls, text, ScriptedProvider, Upstream};
use loomward_protocol::{DatasetClass, EventStream, RootId};
use loomward_service::hostile::{self, raw_hostile, NEEDLES};
use loomward_service::{paths, Config, Service};
use std::path::Path;
use std::sync::Mutex;

/// The service as the tool loop sees it, recording who called which command.
struct Recording(Arc<Service>, Mutex<Vec<(String, String)>>);

impl ViewService for Recording {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        let who = match &ctx.principal {
            loomward_protocol::Principal::Client(c) => c.clone(),
            loomward_protocol::Principal::Owner => "owner".into(),
        };
        self.1
            .lock()
            .unwrap()
            .push((who, request.command.as_ref().to_owned()));
        self.0.call(request, ctx)
    }

    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
        self.0.subscribe(resume)
    }
}

fn owner(service: &Service, command: &str, payload: Value) -> Value {
    let request: RequestEnvelope = serde_json::from_value(json!({
        "protocol":"loomward/3", "request_id":"fixture", "command":command,
        "payload":payload, "deadline_ms":60_000
    }))
    .unwrap();
    match service.call(request, &CallContext::http()) {
        ResponseEnvelope::Ok(ok) => Value::Object(ok.result),
        ResponseEnvelope::Err(err) => panic!("{err:?}"),
    }
}

fn config(state: &Path, grant: Vec<std::path::PathBuf>) -> Config {
    Config {
        state_dir: state.to_owned(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: grant,
    }
}

/// Every table's values, including BLOBs, independently of WAL bytes.
fn dump(path: &Path) -> Vec<(String, Vec<String>)> {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    tables
        .into_iter()
        .map(|name| {
            let mut stmt = db
                .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
                .unwrap();
            let columns = stmt.column_count();
            let mut rows: Vec<_> = stmt
                .query_map([], |row| {
                    Ok((0..columns)
                        .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                        .collect::<Vec<_>>()
                        .join("|"))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows.sort();
            (name, rows)
        })
        .collect()
}

/// Every string and object key of a value: (path, text, is_key).
fn walk<'a>(v: &'a Value, path: &str, out: &mut Vec<(String, &'a str, bool)>) {
    match v {
        Value::String(s) => out.push((path.to_owned(), s, false)),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .for_each(|(i, x)| walk(x, &format!("{path}.{i}"), out)),
        Value::Object(m) => m.iter().for_each(|(k, x)| {
            out.push((format!("{path}.{k}"), k, true));
            walk(x, &format!("{path}.{k}"), out);
        }),
        _ => {}
    }
}

fn local() -> ProviderProfile {
    ProviderProfile {
        base_url: "http://127.0.0.1:11434/v1".into(),
        model: "synthetic-model".into(),
    }
}

/// What the model gets for a tool, with a handle from the first find of the turn.
fn args_for(tool: &str, handle: &str) -> String {
    match tool {
        "find" => json!({"text": "hz00"}).to_string(),
        "describe" => json!({"handle": handle}).to_string(),
        other => panic!("tool `{other}` has no hostile-name case: add one to this test"),
    }
}

/// The fields a name or path is allowed to travel in, in a tool message.
const DATA_FIELDS: &[&str] = &["name", "path", "extension", "modified_at"];

#[test]
fn hostile_names_reach_the_model_only_as_cleaned_data_and_no_tool_reaches_a_write() {
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let tmp = tempfile::tempdir_in(paths::test_roots_base()).unwrap();
    let state = tmp.path().join("state");
    let root = tmp.path().join("lab");
    loomward_service::lab::register(
        &state,
        &root,
        "ask",
        "S",
        &format!("sha256:{}", "00".repeat(32)),
    )
    .unwrap();
    let planted = hostile::plant(&root);
    let service = Service::open(config(&state, vec![root.canonicalize().unwrap()])).unwrap();
    let root_id = owner(&service, "roots.list", json!({}))["roots"][0]["root_id"].clone();
    let job = owner(
        &service,
        "scan.start",
        json!({"root_id":root_id,"mode":"full"}),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = owner(&service, "jobs.get", json!({"job_id":job["job"]["job_id"]}));
        if status["job"]["state"] == "completed" {
            break;
        }
        assert_ne!(status["job"]["state"], "failed", "{status}");
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(10));
    }
    service
        .grant_client(
            "ask:local",
            &[RootId::new(root_id.as_str().unwrap()).unwrap()],
            &["path", "size", "modified"],
            None,
        )
        .unwrap();
    drop(service);
    hostile::inject(&state.join("catalog.db"), &planted);

    let service = Arc::new(Service::open(config(&state, vec![])).unwrap());
    let view = Arc::new(Recording(service.clone(), Mutex::default()));
    // Opening reconciles revisions once; from the first read on, nothing may change.
    let warm = serde_json::from_value(json!({
        "protocol":"loomward/3", "request_id":"warm", "command":"search.query",
        "payload":{"root_id":null,"text":"hz00","extension":null,"min_bytes":null,"max_bytes":null,
                   "modified_after":null,"modified_before":null,"kind":"any","limit":1,"cursor":null}
    }))
    .unwrap();
    assert!(matches!(
        service.call(warm, &CallContext::client("ask:local")),
        ResponseEnvelope::Ok(_)
    ));
    let (state_before, catalog_before) = (
        dump(&state.join("state.db")),
        dump(&state.join("catalog.db")),
    );
    view.1.lock().unwrap().clear();

    let chunks = planted.stored.len().div_ceil(19);
    let mut used = std::collections::BTreeSet::new();
    let mut rows_seen = 0;
    for chunk in 0..=chunks + 2 {
        // The last turn is the model going wrong: invented tools, forged handles and node IDs,
        // hostile arguments. Every earlier turn calls every offered tool on one chunk of names.
        let tag = format!("hz{chunk:02}");
        let script = if chunk < chunks {
            let mut round2 = Vec::new();
            for (i, (tool, _)) in TOOLS.iter().enumerate() {
                let handle = format!("h{}", i + 1);
                round2.push(call(&format!("b{i}"), tool, &args_for(tool, &handle)));
            }
            vec![
                Upstream::Reply(calls(vec![call(
                    "a",
                    "find",
                    &json!({ "text": tag }).to_string(),
                )])),
                Upstream::Reply(calls(round2)),
                Upstream::Reply(text("h1 is the first row.")),
            ]
        } else if chunk < chunks + 2 {
            // These shared-corpus files have no ~hzNN tag: exercise extension and parent path.
            let query = if chunk == chunks {
                "ext-case"
            } else {
                "--delete"
            };
            vec![
                Upstream::Reply(calls(vec![call(
                    "a",
                    "find",
                    &json!({"text":query}).to_string(),
                )])),
                Upstream::Reply(calls(vec![call("b", "describe", r#"{"handle":"h1"}"#)])),
                Upstream::Reply(text("h1 is the first row.")),
            ]
        } else {
            vec![
                Upstream::Reply(calls(vec![
                    call(
                        "1",
                        "find",
                        &json!({"text":"\"}],\"x\":\"\u{202e}; rm -rf x"}).to_string(),
                    ),
                    call("2", "delete", r#"{"handle":"h1"}"#),
                    call("3", "\u{202e}find; rm -rf x", "{}"),
                    call("4", "describe", r#"{"handle":"nd_1"}"#),
                    call("5", "describe", "{\"handle\":\"h1\u{202e}\"}"),
                    call("6", "find", r#"{"text":"x","--delete":1}"#),
                    call("7", "find", "not json \u{202e}"),
                    call("8", "describe", r#"{"handle":"h99"}"#),
                ])),
                Upstream::Reply(text("nothing to cite")),
            ]
        };
        let provider = ScriptedProvider::new(script);
        let mut log = Vec::new();
        let answer = ask(view.clone(), &provider, &local(), "what is here?", &mut log)
            .unwrap_or_else(|f| panic!("{:?}", f.error));
        if chunk < chunks {
            rows_seen += answer.handles.len();
        }
        for tool in &answer.tools {
            used.insert(tool.name.clone());
            for (field, s) in [("name", &tool.name), ("detail", &tool.detail)] {
                assert!(
                    !s.chars().any(raw_hostile),
                    "raw character in the tool log's {field}: {s:?}"
                );
            }
        }
        let mut hostile_extension_seen = false;
        let mut hostile_directory_seen = false;
        for request in provider.seen.lock().unwrap().iter() {
            for message in &request.messages {
                match message {
                    Message::Tool { content, .. } => {
                        let value: Value = serde_json::from_str(content).expect("framing intact");
                        let mut found = Vec::new();
                        walk(&value, "", &mut found);
                        for (path, s, is_key) in found {
                            if let Some(c) = s.chars().find(|c| raw_hostile(*c)) {
                                panic!(
                                    "raw U+{:04X} in tool message at {path}: {content}",
                                    c as u32
                                );
                            }
                            let field = path.rsplit('.').next().unwrap();
                            hostile_extension_seen |= field == "extension" && s == r"\u{202e}fdp";
                            hostile_directory_seen |= field == "path"
                                && s.contains(r"proj\u{202e}; rm -rf x")
                                && s.contains("--delete.");
                            for needle in NEEDLES.iter().filter(|n| s.contains(**n)) {
                                assert!(
                                    !is_key && DATA_FIELDS.contains(&field),
                                    "`{needle}` outside a data field at {path}: {content}"
                                );
                            }
                        }
                    }
                    // Only tool messages carry catalogue text: the rest is the fixed prompt, the
                    // owner's question and the model's own calls.
                    Message::System { content } | Message::User { content } => {
                        assert!(NEEDLES.iter().all(|n| !content.contains(n)), "{content}");
                    }
                    Message::Assistant { .. } => {}
                }
            }
        }
        if chunk == chunks {
            assert!(
                hostile_extension_seen,
                "hostile extension never reached the model"
            );
        } else if chunk == chunks + 1 {
            assert!(
                hostile_directory_seen,
                "hostile directory never reached the model"
            );
        }
        assert_eq!(dump(&state.join("state.db")), state_before, "turn {chunk}");
        assert!(
            dump(&state.join("catalog.db")) == catalog_before,
            "turn {chunk} wrote catalog.db"
        );
    }
    // Every offered tool was called, and the model saw the corpus.
    let offered: std::collections::BTreeSet<String> =
        TOOLS.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert!(used.is_superset(&offered), "{used:?} vs {offered:?}");
    assert!(
        rows_seen >= planted.stored.len(),
        "the model saw {rows_seen} of {} names",
        planted.stored.len()
    );
    // No tool path reached a write: only the read commands behind the offered tools, only as the
    // profile's client.
    let reads: Vec<&str> = TOOLS.iter().map(|(_, command)| *command).collect();
    for (who, command) in view.1.lock().unwrap().iter() {
        assert_eq!(who, "ask:local");
        assert!(reads.contains(&command.as_str()), "{command}");
    }
    drop(view);
    drop(service);
    assert_eq!(dump(&state.join("state.db")), state_before);
    assert!(dump(&state.join("catalog.db")) == catalog_before);
}
