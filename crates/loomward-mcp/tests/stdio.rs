use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

struct Server {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    modern: bool,
}

impl Server {
    fn start(state: &std::path::Path, modern: bool, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_loomward-mcp"))
            .args(["--client", "test", "--dataset", "synthetic", "--state-dir"])
            .arg(state)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let line = line.unwrap();
                let v = serde_json::from_str(&line).expect("stdout must be JSON-RPC only");
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            output: rx,
            modern,
        }
    }

    fn raw(&mut self, bytes: &[u8]) -> Value {
        self.write(bytes);
        self.output
            .recv_timeout(Duration::from_secs(30))
            .expect("one response before timeout")
    }

    fn write(&mut self, bytes: &[u8]) {
        let input = self.input.as_mut().unwrap();
        input.write_all(bytes).unwrap();
        input.flush().unwrap();
    }

    fn call(&mut self, method: &str, mut params: Value) -> Value {
        if self.modern {
            params["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            });
        }
        let mut bytes = json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})
            .to_string()
            .into_bytes();
        bytes.push(b'\n');
        self.raw(&bytes)
    }

    fn handshake(&mut self) {
        if self.modern {
            let v = self.call("server/discover", json!({}));
            assert_eq!(
                v["result"]["supportedVersions"],
                json!(["2026-07-28", "2025-11-25"])
            );
            assert_eq!(v["result"]["resultType"], "complete");
        } else {
            let v = self.call("initialize", json!({"protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"fixture","version":"1"}}));
            assert_eq!(v["result"]["protocolVersion"], "2025-11-25");
            self.write(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
        }
    }

    fn tool(&mut self, name: &str, args: Value) -> Value {
        self.call("tools/call", json!({"name":name, "arguments":args}))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.input.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                self.child.kill().unwrap();
                self.child.wait().unwrap();
                panic!("stdio server failed to exit at EOF");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn both_eras_discover_and_list_exactly_two_read_tools_without_opening_state() {
    let tmp = tempfile::tempdir().unwrap();
    for modern in [false, true] {
        let state = tmp.path().join(if modern { "modern" } else { "legacy" });
        let mut server = Server::start(&state, modern, &[]);
        server.handshake();
        let v = server.call("tools/list", json!({}));
        let names: Vec<_> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["find", "describe"]);
        assert!(server.call("ping", json!({})).get("result").is_some());
        assert_eq!(
            server.tool("scan.start", json!({}))["error"]["code"],
            -32602
        );
        assert!(
            !state.exists(),
            "discovery and hidden names must not open state"
        );
    }
}

#[test]
fn both_eras_reject_bad_frames_ids_metadata_and_tool_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    for modern in [false, true] {
        let state = tmp.path().join(if modern { "modern" } else { "legacy" });
        let mut server = Server::start(&state, modern, &[]);
        server.handshake();
        for raw in [
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"method\":\"tools/call\"}\n"
                .to_string(),
            "{\"a\":{\"x\":1,\"x\":2}}\n".to_string(),
            "{\"x\":NaN}\n".to_string(),
            "{\"x\":1e999}\n".to_string(),
            format!("{}0{}\n", "[".repeat(33), "]".repeat(33)),
        ] {
            assert_eq!(server.raw(raw.as_bytes())["error"]["code"], -32700);
        }
        let mut oversized = vec![b' '; 65_536];
        oversized.push(b'\n');
        assert_eq!(server.raw(&oversized)["error"]["code"], -32600);
        for id in [
            Value::Null,
            json!(true),
            json!(1.5),
            json!(9_007_199_254_740_992_u64),
            json!(""),
            json!("x".repeat(129)),
            json!([]),
        ] {
            let raw = format!("{}\n", json!({"jsonrpc":"2.0", "id":id, "method":"ping"}));
            let v = server.raw(raw.as_bytes());
            assert_eq!(v["error"]["code"], -32600);
            assert!(v.get("id").is_none());
        }
        assert_eq!(
            server.call("tools/list", json!({"cursor":"x"}))["error"]["code"],
            -32602
        );
        assert_eq!(
            server.call("tools/call", json!({"name":"find","principal":"mcp:other"}))["error"]
                ["code"],
            -32602
        );
        for (name, args) in [
            ("find", json!({"text":"report", "execute":true})),
            ("find", json!({"text":"x".repeat(257)})),
            ("find", json!({"text":7})),
            ("describe", json!({"node_id":"invented"})),
        ] {
            assert_eq!(server.tool(name, args)["result"]["isError"], true);
        }
        // Notifications never execute a request or produce an unsolicited response.
        server.write(b"{\"jsonrpc\":\"2.0\",\"method\":\"tools/call\",\"params\":{\"name\":\"find\",\"arguments\":{\"text\":\"report\"}}}\n");
        assert!(server.call("ping", json!({})).get("result").is_some());
        assert!(!state.exists());
    }
}

#[test]
fn protocol_eras_do_not_lend_metadata_or_skip_legacy_initialization() {
    let tmp = tempfile::tempdir().unwrap();
    let mut server = Server::start(tmp.path(), false, &[]);
    assert_eq!(
        server.call("tools/list", json!({}))["error"]["code"],
        -32602
    );
    assert_eq!(
        server.call("server/discover", json!({}))["error"]["code"],
        -32602
    );
    let meta = |version| {
        json!({"_meta":{
            "io.modelcontextprotocol/protocolVersion":version,
            "io.modelcontextprotocol/clientCapabilities":{}
        }})
    };
    assert_eq!(
        server.call("ping", meta("2099-01-01"))["error"]["code"],
        -32022
    );
    assert!(server
        .call("server/discover", meta("2026-07-28"))
        .get("result")
        .is_some());
    assert_eq!(server.call("ping", json!({}))["error"]["code"], -32602);
    server.call("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}));
    assert_eq!(server.call("ping", json!({}))["error"]["code"], -32602);
    server.write(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
    assert!(server.call("ping", json!({})).get("result").is_some());
}

#[test]
fn absent_store_is_a_tool_error_without_creating_state_and_rate_budget_is_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let mut server = Server::start(tmp.path(), true, &[]);
    for _ in 0..120 {
        let v = server.tool("find", json!({"text":"report"}));
        assert_eq!(v["result"]["isError"], true);
        assert!(
            v["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("no data here yet"),
            "{v}"
        );
    }
    let v = server.tool("find", json!({"text":"report"}));
    assert!(
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("budget exceeded"),
        "{v}"
    );
    assert!(!tmp.path().join("ask.db").exists());
    // A client host never creates a store or its lock where none exists.
    assert!(!tmp.path().join("state.db").exists());
    assert!(!tmp.path().join("lock").exists());
}

#[test]
fn cli_usage_errors_are_stderr_only_and_exit_two() {
    for args in [
        vec![],
        vec!["--client", "Upper"],
        vec!["--client", "with.dot"],
        vec!["--client", ""],
        vec![
            "--client",
            "test",
            "--state-dir",
            "synthetic-state",
            "--dataset",
            "personal",
        ],
        vec!["--client", "test", "--dataset", "unknown"],
        vec!["--client", "test", "--idle-release-secs", "-1"],
        vec!["--client", "test", "--idle-release-secs", "1.5"],
        vec!["--client", "test", "--state-dir"],
        vec!["--client", "test", "--unknown"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_loomward-mcp"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8(out.stderr)
            .unwrap()
            .contains("usage: loomward-mcp"));
    }
}

#[cfg(windows)]
mod live {
    use super::*;
    use loomward_protocol::{
        CallContext, DatasetClass, RequestEnvelope, ResponseEnvelope, RootId, ViewService,
    };
    use loomward_service::{paths::lock_state_dir, Config, Service};
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    struct Fixture {
        _tmp: tempfile::TempDir,
        state: PathBuf,
        expected: Value,
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

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
        let state = tmp.path().join("state");
        let root = tmp.path().join("lab");
        loomward_service::lab::register(
            &state,
            &root,
            "mcp",
            "S",
            &format!("sha256:{}", "00".repeat(32)),
        )
        .unwrap();
        for i in 0..25 {
            std::fs::write(root.join(format!("report-{i:02}.txt")), b"synthetic").unwrap();
        }
        let service = Service::open(Config {
            state_dir: state.clone(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![root.canonicalize().unwrap()],
        })
        .unwrap();
        let roots = owner(&service, "roots.list", json!({}));
        let root = roots["roots"][0]["root_id"].clone();
        let job = owner(
            &service,
            "scan.start",
            json!({"root_id":root,"mode":"full"}),
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
                "mcp:test",
                &[RootId::new(root.as_str().unwrap()).unwrap()],
                &["size"],
                None,
            )
            .unwrap();
        let expected = owner(
            &service,
            "search.query",
            json!({"root_id":null,"text":"report","extension":null,
            "min_bytes":null,"max_bytes":null,"modified_after":null,"modified_before":null,
            "kind":"any","limit":20,"cursor":null}),
        );
        drop(service);
        Fixture {
            _tmp: tmp,
            state,
            expected,
        }
    }

    // Compare values of every table, including BLOBs and sqlite_sequence, independently of WAL bytes.
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

    #[test]
    fn both_eras_find_describe_project_grants_and_leave_every_state_table_unchanged() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        let before = dump(&f.state.join("state.db"));
        assert!(!f.state.join("ask.db").exists());
        for modern in [false, true] {
            let mut server = Server::start(&f.state, modern, &[]);
            server.handshake();
            let find = server.tool("find", json!({"text":"report"}));
            assert_eq!(find["result"]["isError"], false, "{find}");
            // Opening reconciles catalogue revisions/sequences. Reads after that open must not
            // change any table; state.db above is compared across the entire launch as required.
            let catalog_before = dump(&f.state.join("catalog.db"));
            assert_eq!(
                server.tool("find", json!({"text":"report"}))["result"]["isError"],
                false
            );
            let data = &find["result"]["structuredContent"];
            let rows = data["rows"].as_array().unwrap();
            assert_eq!(rows.len(), 20);
            assert_eq!(data["more"], true);
            assert!(!data.to_string().contains("node_id"));
            assert!(!data.to_string().contains("nd_"));
            for (row, expected) in rows.iter().zip(f.expected["hits"].as_array().unwrap()) {
                assert_eq!(row["row"]["name"], expected["row"]["name"]);
                assert_eq!(
                    row["row"]["logical_bytes"],
                    expected["row"]["logical_bytes"]
                );
                assert!(row.get("path").is_none());
                assert!(row["row"].get("modified_at").is_none());
                assert!(row["row"].get("attributes").is_none());
            }
            let described = server.tool("describe", json!({"reference":rows[0]["reference"]}));
            assert_eq!(described["result"]["isError"], false, "{described}");
            assert_eq!(
                described["result"]["structuredContent"]["row"],
                rows[0]["row"]
            );
            assert!(!described["result"].to_string().contains("node_id"));
            for name in [
                "scan.start",
                "grant_client",
                "copy",
                "approve",
                "delete",
                "apps",
                "plan_archive",
            ] {
                assert_eq!(server.tool(name, json!({}))["error"]["code"], -32602);
            }
            assert_eq!(dump(&f.state.join("state.db")), before);
            assert_eq!(dump(&f.state.join("catalog.db")), catalog_before);
            assert!(!f.state.join("ask.db").exists());
            drop(server);
            assert_eq!(dump(&f.state.join("state.db")), before);
            assert_eq!(dump(&f.state.join("catalog.db")), catalog_before);
        }
    }

    #[test]
    fn busy_then_success_and_idle_release_expires_references_without_reuse() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        let lock = lock_state_dir(&f.state).unwrap();
        let mut server = Server::start(&f.state, true, &["--idle-release-secs", "1"]);
        let busy = server.tool("find", json!({"text":"report"}));
        assert_eq!(busy["result"]["isError"], true, "{busy}");
        assert_eq!(
            busy["result"]["content"][0]["text"],
            "busy: Loomward is open on this machine; try again after it closes"
        );
        drop(lock);
        let find = server.tool("find", json!({"text":"report"}));
        assert_eq!(find["result"]["isError"], false, "{find}");
        let old = find["result"]["structuredContent"]["rows"][0]["reference"].clone();
        assert!(lock_state_dir(&f.state).unwrap_err().starts_with("busy:"));
        // Leave a partial frame in the pipe: it must not hold the state lock past idle.
        server.write(b" ");
        let deadline = Instant::now() + Duration::from_secs(15);
        let idle_lock = loop {
            if let Ok(lock) = lock_state_dir(&f.state) {
                break lock;
            }
            assert!(
                Instant::now() < deadline,
                "idle server did not release its lock"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        drop(idle_lock);
        let expired = server.tool("describe", json!({"reference":old}));
        assert_eq!(expired["result"]["isError"], true, "{expired}");
        assert_eq!(
            expired["result"]["content"][0]["text"],
            "unknown reference; call find again"
        );
        let fresh = server.tool("find", json!({"text":"report"}));
        assert_ne!(
            fresh["result"]["structuredContent"]["rows"][0]["reference"],
            old
        );
        assert_eq!(
            server.tool("describe", json!({"reference":old}))["result"]["isError"],
            true
        );
    }

    #[test]
    fn zero_idle_timeout_keeps_the_lock_and_revocation_refuses_existing_references() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        let mut server = Server::start(&f.state, true, &["--idle-release-secs", "0"]);
        let found = server.tool("find", json!({"text":"report"}));
        assert_eq!(found["result"]["isError"], false, "{found}");
        let reference = found["result"]["structuredContent"]["rows"][0]["reference"].clone();
        std::thread::sleep(Duration::from_millis(1200));
        assert!(lock_state_dir(&f.state).unwrap_err().starts_with("busy:"));
        // Fault injection simulates a grant revoked while a client is alive. The exclusive host
        // lock normally prevents another owner process from changing it concurrently.
        let db = rusqlite::Connection::open(f.state.join("state.db")).unwrap();
        db.execute(
            "UPDATE client_grant SET revoked_at_ns=1 WHERE client='mcp:test'",
            [],
        )
        .unwrap();
        // Re-grant without an intervening refusal: the epoch itself must expire the reference.
        db.execute("INSERT INTO client_grant(client,roots_json,fields_json,created_at_ns,durable_roots)
            SELECT client,roots_json,fields_json,created_at_ns+1,1 FROM client_grant WHERE client='mcp:test'", []).unwrap();
        let expired = server.tool("describe", json!({"reference":reference}));
        assert_eq!(
            expired["result"]["content"][0]["text"],
            "unknown reference; call find again"
        );
        let fresh = server.tool("find", json!({"text":"report"}));
        assert_eq!(fresh["result"]["isError"], false, "{fresh}");
        assert_ne!(
            fresh["result"]["structuredContent"]["rows"][0]["reference"],
            reference
        );
        db.execute(
            "UPDATE client_grant SET revoked_at_ns=1 WHERE client='mcp:test'",
            [],
        )
        .unwrap();
        let refused = server.tool(
            "describe",
            json!({"reference":fresh["result"]["structuredContent"]["rows"][0]["reference"]}),
        );
        assert_eq!(refused["result"]["isError"], true);
        assert!(refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("permission_denied"));
        assert!(refused["result"].get("structuredContent").is_none());
        // A refused call releases the lock at once, even with no idle release configured.
        drop(lock_state_dir(&f.state).expect("released after a refused call"));
        drop(db);
        drop(server);
        assert!(lock_state_dir(&f.state).is_ok());
    }
}
