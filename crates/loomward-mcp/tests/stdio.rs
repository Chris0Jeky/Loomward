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
        Self::start_client(state, modern, "test", extra)
    }

    fn start_client(state: &std::path::Path, modern: bool, client: &str, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_loomward-mcp"))
            .args(["--client", client, "--dataset", "synthetic", "--state-dir"])
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
fn both_eras_discover_and_list_eight_read_tools_without_opening_state() {
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
        assert_eq!(
            names,
            [
                "find",
                "describe",
                "volumes",
                "roots",
                "largest",
                "breakdown",
                "clutter",
                "plan_archive"
            ]
        );
        for tool in v["result"]["tools"].as_array().unwrap() {
            assert!(tool["description"]
                .as_str()
                .unwrap()
                .contains("third-party text"));
            assert!(tool["description"]
                .as_str()
                .unwrap()
                .contains("shell string"));
        }
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
    use loomward_service::{
        paths::{lock_state_dir, unwant_lock, want_lock},
        Config, Service,
    };
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    struct Fixture {
        _tmp: tempfile::TempDir,
        state: PathBuf,
        lab: PathBuf,
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
        fixture_with(|_| {})
    }

    /// `populate` adds synthetic entries to the lab root before the scan.
    fn fixture_with(populate: impl FnOnce(&Path)) -> Fixture {
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
        std::fs::write(root.join("control-\u{202e}\u{2028}.txt"), b"synthetic").unwrap();
        std::fs::write(root.join("Cargo.toml"), b"synthetic").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join("target").join("build.bin"), b"synthetic").unwrap();
        populate(&root);
        let lab = root.clone();
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
        for (client, fields) in [
            (
                "all",
                vec!["path", "size", "modified", "attributes", "volumes"],
            ),
            ("names", vec![]),
            ("modified", vec!["size", "modified", "volumes"]),
            ("attributes", vec!["size", "attributes", "volumes"]),
            ("sizes-volumes", vec!["size", "volumes"]),
            ("volumes-only", vec!["volumes"]),
        ] {
            service
                .grant_client(
                    &format!("mcp:{client}"),
                    &[RootId::new(root.as_str().unwrap()).unwrap()],
                    &fields,
                    None,
                )
                .unwrap();
        }
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
            lab,
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
            let db = rusqlite::Connection::open(f.state.join("catalog.db")).unwrap();
            db.execute("UPDATE dir SET dirty_rev=agg_valid_rev+1", [])
                .unwrap();
            drop(db);
            let catalog_before = dump(&f.state.join("catalog.db"));
            assert_eq!(
                server.tool("find", json!({"text":"report"}))["result"]["isError"],
                false
            );
            assert!(
                dump(&f.state.join("catalog.db")) == catalog_before,
                "client find repaired catalog.db"
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
    fn busy_then_success_and_idle_release_preserves_references() {
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
        let described = server.tool("describe", json!({"reference":old}));
        assert_eq!(described["result"]["isError"], false, "{described}");
        let fresh = server.tool("find", json!({"text":"report"}));
        assert_eq!(
            fresh["result"]["structuredContent"]["rows"][0]["reference"],
            old
        );
    }

    #[test]
    fn space_tools_are_scoped_field_gated_and_archive_is_read_only() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        let before = dump(&f.state.join("state.db"));
        let mut server = Server::start(&f.state, true, &[]);
        let roots = server.tool("roots", json!({}));
        assert_eq!(roots["result"]["isError"], false, "{roots}");
        let reference = &roots["result"]["structuredContent"]["roots"][0]["reference"];
        for (name, args) in [
            ("largest", json!({"reference": reference, "limit": 3})),
            (
                "breakdown",
                json!({"reference": reference, "by":"extension", "limit": 2}),
            ),
            ("clutter", json!({"root_reference":reference, "limit": 5})),
        ] {
            let value = server.tool(name, args);
            assert_eq!(value["result"]["isError"], false, "{value}");
            let text = value["result"]["structuredContent"].to_string();
            for secret in ["node_id", "rt_", "cursor"] {
                assert!(!text.contains(secret), "{value}");
            }
            assert_eq!(dump(&f.state.join("state.db")), before);
        }
        for (name, args) in [
            ("volumes", json!({})),
            (
                "plan_archive",
                json!({"references":[reference],"destination_volume":"v1"}),
            ),
            ("breakdown", json!({"reference":reference,"by":"age_band"})),
        ] {
            let value = server.tool(name, args);
            assert_eq!(value["result"]["isError"], true, "{value}");
            assert!(
                value["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("does not disclose"),
                "{value}"
            );
            assert_eq!(dump(&f.state.join("state.db")), before);
        }
        assert_eq!(server.tool("largest", json!({}))["result"]["isError"], true);
        assert_eq!(
            server.tool("largest", json!({"reference":"r99999"}))["result"]["isError"],
            true
        );
        assert!(!f.state.join("ask.db").exists());
    }

    fn data(server: &mut Server, state: &Path, name: &str, args: Value) -> Value {
        let before = dump(&state.join("state.db"));
        let value = server.tool(name, args);
        assert_eq!(
            dump(&state.join("state.db")),
            before,
            "{name} changed state.db"
        );
        assert!(!state.join("ask.db").exists());
        assert_eq!(value["result"]["isError"], false, "{value}");
        let out = value["result"]["structuredContent"].clone();
        for secret in ["node_id", "rt_", "cursor"] {
            assert!(!out.to_string().contains(secret), "{value}");
        }
        let text: Value =
            serde_json::from_str(value["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, out);
        out
    }

    fn set_space(state: &Path, capacity: Option<u64>, free: Option<u64>, online: bool) {
        let db = rusqlite::Connection::open(state.join("catalog.db")).unwrap();
        db.execute("UPDATE volume SET capabilities_json=?1,online=?2", rusqlite::params![json!({"space":{"capacity_bytes":capacity.map(|n| n.to_string()),"free_bytes":free.map(|n| n.to_string())}}).to_string(),online]).unwrap();
    }

    #[test]
    fn archive_arithmetic_warnings_optional_fields_and_untrusted_names_over_stdio() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        // Fault injection: catalogue observations only, never real placeholder/reparse effects.
        let db = rusqlite::Connection::open(f.state.join("catalog.db")).unwrap();
        db.execute(
            "UPDATE file SET attrs=attrs|1024|262144 WHERE name LIKE 'control-%'",
            [],
        )
        .unwrap();
        drop(db);
        for (client, modified, attributes) in [
            ("all", true, true),
            ("sizes-volumes", false, false),
            ("modified", true, false),
            ("attributes", false, true),
        ] {
            set_space(
                &f.state,
                Some(1_000_000_000_000),
                Some(10_000_000_009),
                true,
            );
            let mut server = Server::start_client(&f.state, true, client, &[]);
            let roots = data(&mut server, &f.state, "roots", json!({}));
            let root = roots["roots"][0]["reference"].clone();
            let volumes = data(&mut server, &f.state, "volumes", json!({}));
            let destination = volumes["volumes"][0]["volume_key"].clone();
            // The lab volume is internal: `external` is absent (bus unknown) or false (bus known, not USB).
            assert_ne!(volumes["volumes"][0].get("external"), Some(&json!(true)));
            let found = data(&mut server, &f.state, "find", json!({"text":"control-"}));
            let item = found["rows"][0]["reference"].clone();
            assert_eq!(
                found["rows"][0]["row"]["name"],
                "control-\\u202E\\u2028.txt"
            );
            let described = data(&mut server, &f.state, "describe", json!({"reference":item}));
            assert_eq!(found["rows"][0]["row"]["name"], described["row"]["name"]);
            assert_eq!(
                found["rows"][0]["row"]["attributes"],
                described["row"]["attributes"]
            );
            assert_eq!(
                found["rows"][0]["row"]["modified_at"],
                described["row"]["modified_at"]
            );
            let largest = data(
                &mut server,
                &f.state,
                "largest",
                json!({"reference":root,"limit":30}),
            );
            assert!(largest["rows"].as_array().unwrap().len() <= 30);
            let clutter = data(
                &mut server,
                &f.state,
                "clutter",
                json!({"root_reference":root}),
            );
            let candidates = clutter["candidates"].as_array().unwrap();
            assert_eq!(candidates.len(), 1, "{clutter}");
            for key in ["reference", "anchor_reference"] {
                data(
                    &mut server,
                    &f.state,
                    "describe",
                    json!({"reference":candidates[0][key]}),
                );
            }
            for by in ["extension_family", "extension"] {
                data(
                    &mut server,
                    &f.state,
                    "breakdown",
                    json!({"reference":root,"by":by}),
                );
            }
            if modified {
                data(
                    &mut server,
                    &f.state,
                    "breakdown",
                    json!({"reference":root,"by":"age_band"}),
                );
            } else {
                let denied = server.tool("breakdown", json!({"reference":root,"by":"age_band"}));
                assert_eq!(denied["result"]["isError"], true, "{denied}");
                assert!(denied["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("does not disclose modified times"));
            }
            let plan = data(
                &mut server,
                &f.state,
                "plan_archive",
                json!({"references":[item],"destination_volume":destination,"folder_name":"Synthetic archive"}),
            );
            assert_eq!(plan["total"]["bytes"], "9");
            assert_eq!(plan["total"]["files"], 1);
            assert_eq!(plan["items"][0]["freshness"]["state"], "stale");
            assert_eq!(plan["space"]["reserve_bytes"], "10000000000");
            assert_eq!(plan["space"]["available_after_reserve_bytes"], "9");
            assert_eq!(plan["space"]["fits"], true);
            assert_eq!(plan["execution"], "owner_approval_required");
            assert_eq!(plan["shape"], "new folder only");
            assert_eq!(plan["items"][0].get("modified_range").is_some(), modified);
            assert_eq!(plan["total"].get("modified_range").is_some(), modified);
            assert_eq!(
                plan["items"][0]
                    .get("attribute_assessment_complete")
                    .is_some(),
                attributes
            );
            assert!(plan["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["code"] == "same_volume"));
            for code in ["cloud_placeholder", "reparse_point"] {
                assert_eq!(
                    plan["warnings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|w| w["code"] == code),
                    attributes,
                    "{plan}"
                );
            }
            drop(server);
        }
        for (capacity, free, online, fits) in [
            (
                Some(1_000_000_000_000),
                Some(10_000_000_008),
                true,
                json!(false),
            ),
            (Some(1000), Some((1u64 << 30) + 9), true, json!(true)),
            (Some(1000), None, true, json!("unknown")),
            (None, Some(1000), false, json!("unknown")),
        ] {
            set_space(&f.state, capacity, free, online);
            let mut server = Server::start_client(&f.state, true, "all", &[]);
            let found = data(&mut server, &f.state, "find", json!({"text":"control-"}));
            let volumes = data(&mut server, &f.state, "volumes", json!({}));
            let plan = data(
                &mut server,
                &f.state,
                "plan_archive",
                json!({"references":[found["rows"][0]["reference"]],"destination_volume":volumes["volumes"][0]["volume_key"]}),
            );
            assert_eq!(plan["space"]["fits"], fits, "{plan}");
            assert_eq!(
                plan["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|w| w["code"] == "offline_source"),
                // Since E0, `online` comes from live root presence, not the stored flag alone.
                volumes["volumes"][0]["online"] == false
            );
        }
    }

    #[test]
    fn every_space_field_is_enforced_by_service_and_invalid_arguments_are_refused() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        for client in ["names", "volumes-only"] {
            let mut server = Server::start_client(&f.state, true, client, &[]);
            let root =
                data(&mut server, &f.state, "roots", json!({}))["roots"][0]["reference"].clone();
            data(&mut server, &f.state, "find", json!({"text":"report"}));
            for (name, args) in [
                ("largest", json!({"reference":root})),
                ("breakdown", json!({"reference":root,"by":"extension"})),
                ("clutter", json!({"root_reference":root})),
                (
                    "plan_archive",
                    json!({"references":[root],"destination_volume":"synthetic"}),
                ),
            ] {
                let before = dump(&f.state.join("state.db"));
                let value = server.tool(name, args);
                assert_eq!(value["result"]["isError"], true, "{value}");
                assert!(
                    value["result"]["content"][0]["text"]
                        .as_str()
                        .unwrap()
                        .contains("does not disclose sizes"),
                    "{value}"
                );
                assert_eq!(dump(&f.state.join("state.db")), before);
            }
        }
        let mut server = Server::start(&f.state, true, &[]);
        let root = data(&mut server, &f.state, "roots", json!({}))["roots"][0]["reference"].clone();
        for (name, args) in [
            ("largest", json!({"reference":root,"limit":31})),
            ("largest", json!({"reference":root,"limit":0})),
            (
                "breakdown",
                json!({"reference":root,"by":"extension","limit":17}),
            ),
            ("breakdown", json!({"reference":root,"by":"unsupported"})),
            ("clutter", json!({"root_reference":root,"limit":51})),
            ("roots", json!({"root_id":"rt_forged"})),
            ("volumes", json!({"anything":true})),
            (
                "plan_archive",
                json!({"references":[],"destination_volume":"x"}),
            ),
            (
                "plan_archive",
                json!({"references":vec!["r1";65],"destination_volume":"x"}),
            ),
            (
                "plan_archive",
                json!({"references":[root,root],"destination_volume":"x"}),
            ),
            (
                "plan_archive",
                json!({"references":[root],"destination_volume":"x","folder_name":"..\\existing"}),
            ),
            (
                "plan_archive",
                json!({"references":[root],"destination_volume":"x","folder_name":"CON"}),
            ),
        ] {
            assert_eq!(server.tool(name, args)["result"]["isError"], true);
        }
    }

    #[test]
    fn partial_sizes_and_oversized_items_are_honest_and_scope_withdrawal_refuses_references() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        set_space(
            &f.state,
            Some(1_000_000_000_000),
            Some(900_000_000_000),
            true,
        );
        let db = rusqlite::Connection::open(f.state.join("catalog.db")).unwrap();
        db.execute(
            "UPDATE dir SET sub_files=200001 WHERE parent_id IS NULL",
            [],
        )
        .unwrap();
        let mut server = Server::start_client(&f.state, true, "all", &[]);
        let root = data(&mut server, &f.state, "roots", json!({}))["roots"][0]["reference"].clone();
        let destination =
            data(&mut server, &f.state, "volumes", json!({}))["volumes"][0]["volume_key"].clone();
        let plan = data(
            &mut server,
            &f.state,
            "plan_archive",
            json!({"references":[root],"destination_volume":destination}),
        );
        assert!(
            plan["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["code"] == "split_item"),
            "{plan}"
        );
        drop(server);
        db.execute("UPDATE dir SET listing_state='incomplete',listing_outcome='partial',sub_complete=0 WHERE parent_id IS NULL",[]).unwrap();
        let mut server = Server::start_client(&f.state, true, "all", &[]);
        let root = data(&mut server, &f.state, "roots", json!({}))["roots"][0]["reference"].clone();
        let plan = data(
            &mut server,
            &f.state,
            "plan_archive",
            json!({"references":[root],"destination_volume":destination}),
        );
        assert!(plan["items"][0]["bytes"].is_null(), "{plan}");
        assert!(plan["total"]["bytes"].is_null(), "{plan}");
        assert_eq!(plan["space"]["fits"], "unknown");
        assert_eq!(plan["total"]["modified_range"]["complete"], false);
        // Fault injection: the next call observes the owner's scope withdrawal, even with the
        // same client-grant epoch. No data from the formerly granted root is returned.
        let state = rusqlite::Connection::open(f.state.join("state.db")).unwrap();
        state
            .execute(
                "UPDATE client_grant SET roots_json='[]' WHERE client='mcp:all'",
                [],
            )
            .unwrap();
        let denied = server.tool("largest", json!({"reference":root}));
        assert_eq!(denied["result"]["isError"], true, "{denied}");
        assert!(denied["result"].get("structuredContent").is_none());
    }

    #[test]
    fn references_cannot_alias_the_same_first_result_after_a_process_restart() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        let mut server = Server::start(&f.state, true, &[]);
        let old = data(&mut server, &f.state, "find", json!({"text":"control-"}))["rows"][0]
            ["reference"]
            .clone();
        drop(server);
        let mut server = Server::start(&f.state, true, &[]);
        let fresh = data(&mut server, &f.state, "find", json!({"text":"control-"}))["rows"][0]
            ["reference"]
            .clone();
        assert_ne!(old, fresh);
        assert_eq!(
            server.tool("describe", json!({"reference":old}))["result"]["isError"],
            true
        );
    }

    #[test]
    fn rescan_after_idle_makes_references_unknown_and_restart_does_not_accept_them() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        let mut server = Server::start(&f.state, true, &["--idle-release-secs", "1"]);
        let found = data(&mut server, &f.state, "find", json!({"text":"report"}));
        let reference = found["rows"][0]["reference"].clone();
        let deadline = Instant::now() + Duration::from_secs(15);
        let lock = loop {
            if let Ok(lock) = lock_state_dir(&f.state) {
                break lock;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(50));
        };
        let service = Service::open(Config {
            state_dir: f.state.clone(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        let root = owner(&service, "roots.list", json!({}))["roots"][0]["root_id"].clone();
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
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(service);
        drop(lock);
        let refused = server.tool("describe", json!({"reference":reference}));
        assert_eq!(
            refused["result"]["content"][0]["text"],
            "unknown reference; call find again"
        );
        let fresh = data(&mut server, &f.state, "find", json!({"text":"report"}));
        assert_ne!(fresh["rows"][0]["reference"], reference);
        drop(server);
        let mut restarted = Server::start(&f.state, true, &[]);
        assert_eq!(
            restarted.tool(
                "describe",
                json!({"reference":fresh["rows"][0]["reference"]})
            )["result"]["content"][0]["text"],
            "unknown reference; call find again"
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

    fn codes(plan: &Value) -> Vec<&str> {
        plan["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["code"].as_str().unwrap())
            .collect()
    }

    // Root, destination and a plan of the root folder, with catalog.db unchanged across the plan.
    fn plan_root(server: &mut Server, state: &Path) -> Value {
        let root = data(server, state, "roots", json!({}))["roots"][0]["reference"].clone();
        let destination =
            data(server, state, "volumes", json!({}))["volumes"][0]["volume_key"].clone();
        let catalog = dump(&state.join("catalog.db"));
        let plan = data(
            server,
            state,
            "plan_archive",
            json!({"references":[root],"destination_volume":destination}),
        );
        assert!(
            dump(&state.join("catalog.db")) == catalog,
            "plan_archive changed catalog.db"
        );
        assert_eq!(plan["items"][0]["reference"], root);
        assert_eq!(plan["items"][0]["kind"], "dir");
        plan
    }

    #[test]
    fn a_renamed_lab_root_reads_as_offline_source_and_offline_destination() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        // The stored flag says online: only live presence can make the volume offline.
        set_space(
            &f.state,
            Some(1_000_000_000_000),
            Some(900_000_000_000),
            true,
        );
        std::fs::rename(&f.lab, f.lab.with_file_name("lab-moved")).unwrap();
        let mut server = Server::start_client(&f.state, true, "all", &[]);
        let found = data(&mut server, &f.state, "find", json!({"text":"control-"}));
        let volumes = data(&mut server, &f.state, "volumes", json!({}));
        assert_eq!(volumes["volumes"][0]["online"], false, "{volumes}");
        let plan = data(
            &mut server,
            &f.state,
            "plan_archive",
            json!({"references":[found["rows"][0]["reference"]],"destination_volume":volumes["volumes"][0]["volume_key"]}),
        );
        let codes = codes(&plan);
        for code in ["offline_source", "offline_destination", "same_volume"] {
            assert!(codes.contains(&code), "{plan}");
        }
        assert_eq!(plan["space"]["fits"], "unknown");
    }

    #[test]
    fn a_healthy_folder_walk_is_complete_and_sees_a_nested_placeholder() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        set_space(
            &f.state,
            Some(1_000_000_000_000),
            Some(900_000_000_000),
            true,
        );
        let mut server = Server::start_client(&f.state, true, "all", &[]);
        let started = Instant::now();
        let plan = plan_root(&mut server, &f.state);
        let item = &plan["items"][0];
        let range = &item["modified_range"];
        // On a starved host the plan's own five-second detail budget can trip; say so.
        let took = started.elapsed();
        assert_eq!(range["complete"], true, "plan_root took {took:?}: {plan}");
        let (from, to) = (
            range["from"].as_str().unwrap(),
            range["to"].as_str().unwrap(),
        );
        assert!(from <= to, "{plan}");
        assert_eq!(plan["total"]["modified_range"], *range);
        assert_eq!(item["attribute_assessment_complete"], true);
        assert_eq!(item["files"], 28);
        let seen = codes(&plan);
        assert!(!seen.contains(&"cloud_placeholder") && !seen.contains(&"reparse_point"));
        drop(server);
        // Fault injection, catalogue only: a placeholder one folder down, under target/, is
        // reachable only through the descendant walk.
        let db = rusqlite::Connection::open(f.state.join("catalog.db")).unwrap();
        assert_eq!(
            db.execute(
                "UPDATE file SET attrs=attrs|262144 WHERE name='build.bin'",
                []
            )
            .unwrap(),
            1
        );
        drop(db);
        let mut server = Server::start_client(&f.state, true, "all", &[]);
        let plan = plan_root(&mut server, &f.state);
        let warning = plan["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["code"] == "cloud_placeholder")
            .unwrap_or_else(|| panic!("{plan}"));
        assert_eq!(warning["reference"], plan["items"][0]["reference"]);
        assert!(!codes(&plan).contains(&"reparse_point"), "{plan}");
    }

    #[test]
    fn a_real_junction_descendant_warns_reparse_point() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture_with(|lab| {
            let elsewhere = lab.with_file_name("junction-target");
            std::fs::create_dir(&elsewhere).unwrap();
            std::fs::create_dir(lab.join("nested")).unwrap();
            // A junction needs no privilege; the scan never follows it.
            let made = Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(lab.join("nested").join("link"))
                .arg(&elsewhere)
                .output()
                .unwrap();
            assert!(made.status.success(), "{made:?}");
        });
        set_space(
            &f.state,
            Some(1_000_000_000_000),
            Some(900_000_000_000),
            true,
        );
        let mut server = Server::start_client(&f.state, true, "attributes", &[]);
        let plan = plan_root(&mut server, &f.state);
        assert!(codes(&plan).contains(&"reparse_point"), "{plan}");
        assert!(!codes(&plan).contains(&"cloud_placeholder"), "{plan}");
    }

    #[test]
    fn a_folder_walk_past_32_pages_is_marked_incomplete() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        // 40 subfolders plus target/ and the root itself: more pages than the plan's 32.
        let f = fixture_with(|lab| {
            for i in 0..40 {
                std::fs::create_dir(lab.join(format!("part-{i:02}"))).unwrap();
            }
        });
        set_space(
            &f.state,
            Some(1_000_000_000_000),
            Some(900_000_000_000),
            true,
        );
        let mut server = Server::start_client(&f.state, true, "all", &[]);
        let plan = plan_root(&mut server, &f.state);
        assert_eq!(
            plan["items"][0]["modified_range"]["complete"], false,
            "{plan}"
        );
        assert_eq!(plan["items"][0]["attribute_assessment_complete"], false);
        assert_eq!(plan["total"]["modified_range"]["complete"], false);
        // The page budget bounds detail only: the folder's own size stays known.
        assert_eq!(plan["total"]["files"], 28);
    }

    fn busy(server: &mut Server) {
        let v = server.tool("find", json!({"text":"report"}));
        assert_eq!(
            v["result"]["content"][0]["text"],
            "busy: Loomward is open on this machine; try again after it closes",
            "{v}"
        );
    }

    #[test]
    fn a_holder_with_zero_idle_release_yields_to_the_desktop_marker() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        let before = dump(&f.state.join("state.db"));
        let mut server = Server::start(&f.state, true, &["--idle-release-secs", "0"]);
        data(&mut server, &f.state, "find", json!({"text":"report"}));
        assert!(lock_state_dir(&f.state).unwrap_err().starts_with("busy:"));
        // What the desktop does: mark, retry for up to 15 s, take the lock, unmark. The holder
        // sees the marker between calls; the client sends nothing meanwhile.
        want_lock(&f.state).unwrap();
        let started = Instant::now();
        let desktop = loop {
            if let Ok(lock) = lock_state_dir(&f.state) {
                break lock;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "the holder ignored the marker"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        unwant_lock(&f.state);
        busy(&mut server);
        drop(desktop);
        // The yield lasts about 30 s: with the lock free and no marker, a polling client still
        // cannot take the lock back from a desktop that is between its retries.
        for _ in 0..3 {
            busy(&mut server);
            drop(lock_state_dir(&f.state).expect("the yielding server took the lock back"));
        }
        assert_eq!(dump(&f.state.join("state.db")), before);
        assert!(!f.state.join("ask.db").exists());
    }

    #[test]
    fn a_stale_marker_is_ignored_and_a_fresh_one_refuses_the_first_lock() {
        if loomward_windows::enumerate::skip_elevated_test() {
            return;
        }
        let f = fixture();
        want_lock(&f.state).unwrap();
        std::fs::File::options()
            .write(true)
            .open(f.state.join("lock.wanted"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(60))
            .unwrap();
        let mut server = Server::start(&f.state, true, &["--idle-release-secs", "0"]);
        data(&mut server, &f.state, "find", json!({"text":"report"}));
        // Several marker polls later the holder still holds.
        std::thread::sleep(Duration::from_millis(1500));
        assert!(lock_state_dir(&f.state).unwrap_err().starts_with("busy:"));
        data(&mut server, &f.state, "find", json!({"text":"report"}));
        drop(server);
        // A fresh marker: a server that takes the lock for its first call releases it at once.
        want_lock(&f.state).unwrap();
        let mut server = Server::start(&f.state, true, &[]);
        busy(&mut server);
        drop(lock_state_dir(&f.state).expect("the first lock was not released"));
    }
}
