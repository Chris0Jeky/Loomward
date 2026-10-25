//! L1b: every example validates against its `$def` (JSON Schema) and round-trips through its
//! Rust DTO to an equal `serde_json::Value`; every negative fails in both; the schema keeps
//! its structural promises (closed objects, constant-false effects, no path-taking command).

mod common;

use common::registry::roundtrip;
use jsonschema::Validator;
use loomward_protocol::{
    decode_exact, Command, EventEnvelope, EventName, RequestEnvelope, ResponseEnvelope,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

fn contracts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/v3")
}

#[test]
fn sampled_system_cpu_count_is_required_but_nullable() {
    let mut schema = Schema::load();
    let mut value = read(&contracts().join("examples/commands/telemetry.snapshot.result.json"))
        ["system"]
        .clone();
    assert!(value.is_object());
    for cpus in [json!(16), Value::Null] {
        value["cpu"]["logical_cpus"] = cpus;
        assert!(schema.errors("SystemSample", &value).is_empty());
        let decoded: loomward_protocol::SystemSample =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
    value["cpu"].as_object_mut().unwrap().remove("logical_cpus");
    assert!(!schema.errors("SystemSample", &value).is_empty());
    assert!(serde_json::from_value::<loomward_protocol::SystemSample>(value).is_err());
}

#[test]
fn diagnostics_elevated_session_refusal_roundtrips_through_schema_and_dto() {
    let value = json!({
        "outcome": "refused", "bytes_written": null, "refusal": "elevated_session"
    });
    let mut schema = Schema::load();
    assert!(schema.errors("DiagnosticsSaveResult", &value).is_empty());
    assert_eq!(
        roundtrip("DiagnosticsSaveResult", &value).unwrap().unwrap(),
        value
    );
}

#[test]
fn an_unsettled_search_row_has_required_nullable_sizes() {
    let mut row = read(&contracts().join("examples/commands/search.query.result.json"))["hits"][0]
        ["row"]
        .clone();
    row["kind"] = json!("dir");
    row["coverage"] = json!("unknown");
    for field in ["logical_bytes", "allocated_bytes", "files", "dirs"] {
        row[field] = Value::Null;
    }
    let mut schema = Schema::load();
    assert!(schema.errors("EntryRow", &row).is_empty());
    assert_eq!(roundtrip("EntryRow", &row).unwrap().unwrap(), row);
    row.as_object_mut().unwrap().remove("logical_bytes");
    assert!(!schema.errors("EntryRow", &row).is_empty());
    assert!(roundtrip("EntryRow", &row).unwrap().is_err());
}

fn read(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

struct Schema {
    root: Value,
    validators: HashMap<String, Validator>,
}

#[test]
fn client_projection_contracts_preserve_omitted_and_unknown_fields() {
    let mut schema = Schema::load();
    let owner = read(&contracts().join("examples/commands/node.inspect.result.json"));
    let owner_page = read(&contracts().join("examples/commands/search.query.result.json"));
    for mask in 0..16 {
        let mut row = owner["row"].clone();
        for (i, fields) in [
            &["location_hint"][..],
            &["logical_bytes", "allocated_bytes", "files", "dirs"][..],
            &["modified_at"][..],
            &["attributes", "flags"][..],
        ]
        .iter()
        .enumerate()
        {
            if mask & (1 << i) == 0 {
                for field in *fields {
                    row.as_object_mut().unwrap().remove(*field);
                }
            }
        }
        let mut node = json!({"row": row, "root_id": owner["root_id"]});
        let mut hit = owner_page["hits"][0].clone();
        hit["row"] = row.clone();
        if mask & 1 != 0 {
            node["display_path"] = owner["display_path"].clone();
        } else {
            hit.as_object_mut().unwrap().remove("path");
        }
        let mut page = owner_page.clone();
        page["hits"] = json!([hit]);
        for (def, value) in [
            ("ClientEntryRow", &row),
            ("ClientSearchHit", &hit),
            ("ClientNodeDetail", &node),
            ("ClientSearchPage", &page),
        ] {
            assert!(schema.errors(def, value).is_empty(), "{def}: {value}");
            assert_eq!(roundtrip(def, value).unwrap().unwrap(), *value);
            let mut invalid = value.clone();
            invalid["owner_only"] = json!(true);
            assert!(!schema.errors(def, &invalid).is_empty());
            assert!(roundtrip(def, &invalid).unwrap().is_err());
        }
        for (command, value) in [(Command::NodeInspect, node), (Command::SearchQuery, page)] {
            command
                .validate_client_result(value.as_object().unwrap())
                .unwrap();
            let mut invalid = value.clone();
            let row = if command == Command::NodeInspect {
                &mut invalid["row"]
            } else {
                &mut invalid["hits"][0]["row"]
            };
            row.as_object_mut().unwrap().remove("node_id");
            assert!(!schema
                .errors(command.client_result_def().unwrap(), &invalid)
                .is_empty());
            assert!(command
                .validate_client_result(invalid.as_object().unwrap())
                .is_err());
        }
    }
    // Optional disclosure fields keep their value constraints when present.
    for (field, value) in [
        ("attributes", Value::Null),
        ("flags", Value::Null),
        ("logical_bytes", json!(-1)),
        ("modified_at", json!("yesterday")),
    ] {
        let mut row = owner["row"].clone();
        row[field] = value;
        assert!(!schema.errors("ClientEntryRow", &row).is_empty());
        assert!(roundtrip("ClientEntryRow", &row).unwrap().is_err());
    }
    assert!(Command::HealthGet
        .validate_client_result(&Default::default())
        .is_err());
}

impl Schema {
    fn load() -> Self {
        Self {
            root: read(&contracts().join("view-service.schema.json")),
            validators: HashMap::new(),
        }
    }

    fn defs(&self) -> &serde_json::Map<String, Value> {
        self.root["$defs"].as_object().unwrap()
    }

    /// Validates `value` against one `$def` with format checks on (date-time).
    fn errors(&mut self, def: &str, value: &Value) -> Vec<String> {
        if !self.validators.contains_key(def) {
            assert!(self.defs().contains_key(def), "no $def named {def}");
            let mut schema = self.root.clone();
            let obj = schema.as_object_mut().unwrap();
            obj.remove("anyOf");
            obj.insert("$ref".into(), json!(format!("#/$defs/{def}")));
            let validator = jsonschema::draft202012::options()
                .should_validate_formats(true)
                .build(&schema)
                .unwrap_or_else(|e| panic!("{def}: {e}"));
            self.validators.insert(def.into(), validator);
        }
        self.validators[def]
            .iter_errors(value)
            .map(|e| e.to_string())
            .collect()
    }
}

impl Schema {
    /// Schema errors for a whole envelope, with its payload checked against the type
    /// `commands.json` names for the envelope's command or event (semantics.md section 10).
    /// `command` pairs a response with the request it answers.
    fn complete_errors(
        &mut self,
        def: &str,
        value: &Value,
        command: Option<Command>,
    ) -> Vec<String> {
        let mut errors = self.errors(def, value);
        let (payload, payload_def) = match def {
            "RequestEnvelope" => {
                let cmd = value["command"].as_str().and_then(Command::from_name);
                (&value["payload"], cmd.map(|c| c.request_def()))
            }
            "ResponseEnvelope" if value["ok"] == json!(true) => {
                (&value["result"], command.map(|c| c.result_def()))
            }
            "EventEnvelope" => {
                let ev = serde_json::from_value::<EventName>(value["event"].clone()).ok();
                (&value["data"], ev.map(|e| e.data_def()))
            }
            _ => return errors,
        };
        match payload_def {
            Some(d) => errors.extend(self.errors(d, payload)),
            None => errors.push("no payload type for this envelope".into()),
        }
        errors
    }
}

fn files(dir: &str) -> Vec<PathBuf> {
    let mut out: Vec<_> = fs::read_dir(contracts().join("examples").join(dir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    out.sort();
    out
}

fn stem(p: &Path) -> String {
    p.file_name().unwrap().to_str().unwrap().to_string()
}

/// Every valid example in one place: (file label, def, value).
fn valid_examples() -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    for c in Command::ALL {
        for (kind, def) in [("request", c.request_def()), ("result", c.result_def())] {
            let p = contracts().join(format!("examples/commands/{}.{kind}.json", c.as_str()));
            out.push((stem(&p), def.to_string(), read(&p)));
        }
    }
    for e in EventName::ALL {
        let p = contracts().join(format!("examples/events/{}.json", e.as_str()));
        out.push((stem(&p), e.data_def().to_string(), read(&p)));
    }
    for dir in ["envelopes", "dtos"] {
        for p in files(dir) {
            let doc = read(&p);
            out.push((
                stem(&p),
                doc["def"].as_str().unwrap().to_string(),
                doc["value"].clone(),
            ));
        }
    }
    out
}

#[test]
fn example_directories_hold_exactly_the_expected_files() {
    let expect_commands: BTreeSet<_> = Command::ALL
        .iter()
        .flat_map(|c| {
            [
                format!("{}.request.json", c.as_str()),
                format!("{}.result.json", c.as_str()),
            ]
        })
        .collect();
    let have: BTreeSet<_> = files("commands").iter().map(|p| stem(p)).collect();
    assert_eq!(
        have, expect_commands,
        "one request and one result example per command"
    );
    let expect_events: BTreeSet<_> = EventName::ALL
        .iter()
        .map(|e| format!("{}.json", e.as_str()))
        .collect();
    let have: BTreeSet<_> = files("events").iter().map(|p| stem(p)).collect();
    assert_eq!(have, expect_events, "one example per event");
    let have: BTreeSet<_> = files("dtos").iter().map(|p| stem(p)).collect();
    assert_eq!(
        have,
        BTreeSet::from([
            "ClientField.json".to_string(),
            "McpClient.json".to_string(),
            "disclosure-summary.json".to_string(),
            "duplicates-result.json".to_string(),
            "identity-observation-unverified.json".to_string(),
            "search-freshness-restarting.json".to_string(),
            "search-page-federated.json".to_string(),
            "search-request-federated.json".to_string()
        ])
    );
    assert!(
        files("invalid").len() >= 40,
        "the negatives are the point of the suite"
    );
}

#[test]
fn job_counters_are_required_unsigned_exact_integers() {
    let mut schema = Schema::load();
    let job = read(&contracts().join("examples/commands/jobs.get.result.json"))["job"].clone();
    for field in ["seq", "rev"] {
        for valid in [0, 9_007_199_254_740_991_u64] {
            let mut value = job.clone();
            value[field] = json!(valid);
            assert!(schema.errors("Job", &value).is_empty());
            assert_eq!(roundtrip("Job", &value).unwrap().unwrap(), value);
            assert!(serde_json::from_value::<loomward_protocol::Job>(value).is_ok());
        }
        let mut missing = job.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(!schema.errors("Job", &missing).is_empty());
        assert!(roundtrip("Job", &missing).unwrap().is_err());
        assert!(serde_json::from_value::<loomward_protocol::Job>(missing).is_err());
        for invalid in [
            json!(-1),
            json!(1.5),
            json!(null),
            json!("1"),
            json!(9_007_199_254_740_992_u64),
        ] {
            let mut value = job.clone();
            value[field] = invalid;
            assert!(!schema.errors("Job", &value).is_empty());
            assert!(roundtrip("Job", &value).unwrap().is_err());
            assert!(serde_json::from_value::<loomward_protocol::Job>(value).is_err());
        }
    }
}

#[test]
fn every_def_has_a_rust_type() {
    let schema = Schema::load();
    for def in schema.defs().keys() {
        assert!(
            roundtrip(def, &Value::Null).is_some(),
            "no Rust type registered for $def {def}"
        );
    }
}

#[test]
fn every_enum_value_round_trips() {
    let schema = Schema::load();
    for (def, node) in schema.defs() {
        let Some(values) = node.get("enum").and_then(Value::as_array) else {
            continue;
        };
        for v in values {
            let got = roundtrip(def, v)
                .unwrap()
                .unwrap_or_else(|e| panic!("{def} {v}: {e}"));
            assert_eq!(&got, v, "{def}");
        }
    }
}

#[test]
fn valid_examples_validate_and_round_trip() {
    let mut schema = Schema::load();
    let examples = valid_examples();
    assert_eq!(
        examples.len(),
        Command::ALL.len() * 2
            + EventName::ALL.len()
            + files("envelopes").len()
            + files("dtos").len()
    );
    for (label, def, value) in examples {
        let errors = schema.errors(&def, &value);
        assert!(
            errors.is_empty(),
            "{label} is not a valid {def}: {errors:?}"
        );
        let back = roundtrip(&def, &value)
            .unwrap_or_else(|| panic!("{label}: no Rust type for {def}"))
            .unwrap_or_else(|e| panic!("{label} does not deserialise as {def}: {e}"));
        assert_eq!(
            back, value,
            "{label} changed on the round trip through {def}"
        );
    }
}

/// The Rust verdict for a whole value, including the complete-envelope rule for envelopes.
fn rust_verdict(def: &str, value: &Value, complete: bool) -> Result<(), String> {
    roundtrip(def, value)
        .unwrap_or_else(|| panic!("no Rust type for {def}"))
        .map_err(|e| e.to_string())?;
    if !complete {
        return Ok(());
    }
    match def {
        "EventEnvelope" => decode_exact::<EventEnvelope>(value.clone())
            .and_then(|e| e.validate())
            .map_err(|e| e.message.to_string()),
        "RequestEnvelope" => decode_exact::<RequestEnvelope>(value.clone())
            .and_then(|e| e.validate().map(drop))
            .map_err(|e| e.message.to_string()),
        _ => Ok(()),
    }
}

#[test]
fn invalid_examples_fail_the_schema_and_the_dto() {
    let mut schema = Schema::load();
    let (mut rust_only, mut complete) = (0, 0);
    for p in files("invalid") {
        let doc = read(&p);
        let (def, value) = (doc["def"].as_str().unwrap(), &doc["value"]);
        let flag = |k: &str| doc.get(k) == Some(&json!(true));
        assert!(
            doc["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{} needs a reason",
            stem(&p)
        );
        let schema_errors = if flag("complete") {
            schema.complete_errors(def, value, None)
        } else {
            schema.errors(def, value)
        };
        if flag("rust_only") {
            // JSON Schema cannot express this rule; the runtime check must.
            rust_only += 1;
            assert!(
                schema_errors.is_empty(),
                "{} is marked rust_only but the schema rejects it",
                stem(&p)
            );
        } else {
            assert!(
                !schema_errors.is_empty(),
                "{} passes the schema as {def}",
                stem(&p)
            );
        }
        complete += usize::from(flag("complete"));
        assert!(
            rust_verdict(def, value, flag("complete")).is_err(),
            "{} deserialises as {def} but must not",
            stem(&p)
        );
    }
    assert!(rust_only >= 2 && complete >= 3, "{rust_only} {complete}");
}

/// Complete envelopes: every command's request and ok response and every event, wrapped in a
/// full envelope, validate with their payload discriminated through commands.json (schema
/// and Rust), carry revisions, and round-trip.
#[test]
fn complete_envelopes_validate_for_every_command_and_event() {
    let mut schema = Schema::load();
    let meta = json!({"served_at":"2026-10-09T12:00:00.250Z","elapsed_ms":3,"dataset_class":"synthetic","budget_hit":false,"catalog_rev":"8812","state_rev":"311"});
    for c in Command::ALL {
        let payload =
            read(&contracts().join(format!("examples/commands/{}.request.json", c.as_str())));
        let result =
            read(&contracts().join(format!("examples/commands/{}.result.json", c.as_str())));
        let request = json!({"protocol":"loomward/3","request_id":"r_1","command":c.as_str(),"payload":payload});
        let errs = schema.complete_errors("RequestEnvelope", &request, None);
        assert!(errs.is_empty(), "{} request: {errs:?}", c.as_str());
        let env = decode_exact::<RequestEnvelope>(request.clone()).unwrap();
        assert_eq!(env.validate().unwrap(), *c);
        assert_eq!(serde_json::to_value(&env).unwrap(), request);

        let response = json!({"protocol":"loomward/3","request_id":"r_1","ok":true,"result":result,"meta":meta});
        let errs = schema.complete_errors("ResponseEnvelope", &response, Some(*c));
        assert!(errs.is_empty(), "{} response: {errs:?}", c.as_str());
        match decode_exact::<ResponseEnvelope>(response.clone()).unwrap() {
            ResponseEnvelope::Ok(ok) => ok.validate_for(*c).unwrap(),
            ResponseEnvelope::Err(_) => panic!("ok response decoded as an error"),
        }
    }
    for e in EventName::ALL {
        let data = read(&contracts().join(format!("examples/events/{}.json", e.as_str())));
        let event = json!({"protocol":"loomward/3","epoch":"e_k3J9x2Qa","seq":9,"event":e.as_str(),"at":"2026-10-09T12:00:00.250Z","catalog_rev":null,"state_rev":null,"data":data});
        let errs = schema.complete_errors("EventEnvelope", &event, None);
        assert!(errs.is_empty(), "{} event: {errs:?}", e.as_str());
        let env = decode_exact::<EventEnvelope>(event.clone()).unwrap();
        env.validate().unwrap();
        assert_eq!(serde_json::to_value(&env).unwrap(), event);
    }
}

/// Every object with declared properties is closed.
#[test]
fn objects_with_properties_are_closed() {
    let schema = Schema::load();
    fn walk(path: &str, node: &Value, open: &mut Vec<String>) {
        match node {
            Value::Object(map) => {
                if map.contains_key("properties")
                    && map.get("additionalProperties") != Some(&json!(false))
                {
                    open.push(path.to_string());
                }
                for (k, v) in map {
                    walk(&format!("{path}/{k}"), v, open);
                }
            }
            Value::Array(items) => items
                .iter()
                .enumerate()
                .for_each(|(i, v)| walk(&format!("{path}/{i}"), v, open)),
            _ => {}
        }
    }
    let mut open = Vec::new();
    walk("", &schema.root["$defs"], &mut open);
    assert!(open.is_empty(), "open objects: {open:?}");
}

#[test]
fn every_effect_capability_is_the_constant_false() {
    let schema = Schema::load();
    let effects = &schema.defs()["Capabilities"]["properties"]["effects"];
    let props = effects["properties"].as_object().unwrap();
    assert_eq!(props.len(), 11);
    assert_eq!(effects["required"].as_array().unwrap().len(), props.len());
    for (name, node) in props {
        assert_eq!(node, &json!({"const": false}), "effects.{name}");
    }
}

/// No request carries a path, command line or executable name (invariants 1 and 2): checked
/// on property names across every request def and everything it references.
#[test]
fn no_request_accepts_a_path() {
    let schema = Schema::load();
    const FORBIDDEN: &[&str] = &[
        "path",
        "paths",
        "file",
        "files",
        "file_path",
        "dir",
        "directory",
        "folder",
        "filename",
        "cmdline",
        "command_line",
        "exe",
        "executable",
        "argv",
    ];
    fn props(schema: &Schema, node: &Value, seen: &mut BTreeSet<String>, hits: &mut Vec<String>) {
        match node {
            Value::Object(map) => {
                if let Some(r) = map.get("$ref").and_then(Value::as_str) {
                    let name = r.rsplit('/').next().unwrap().to_string();
                    if seen.insert(name.clone()) {
                        props(schema, &schema.defs()[&name], seen, hits);
                    }
                }
                if let Some(p) = map.get("properties").and_then(Value::as_object) {
                    hits.extend(
                        p.keys()
                            .filter(|k| FORBIDDEN.contains(&k.as_str()))
                            .cloned(),
                    );
                }
                map.values().for_each(|v| props(schema, v, seen, hits));
            }
            Value::Array(items) => items.iter().for_each(|v| props(schema, v, seen, hits)),
            _ => {}
        }
    }
    for c in Command::ALL {
        let mut hits = Vec::new();
        props(
            &schema,
            &schema.defs()[c.request_def()],
            &mut BTreeSet::new(),
            &mut hits,
        );
        assert!(
            hits.is_empty(),
            "{} request has path-like properties {hits:?}",
            c.as_str()
        );
    }
}

/// JSON pointers of every node, root included.
fn pointers(v: &Value, here: &str, out: &mut Vec<String>) {
    out.push(here.to_string());
    match v {
        Value::Object(m) => m.iter().for_each(|(k, c)| {
            pointers(
                c,
                &format!("{here}/{}", k.replace('~', "~0").replace('/', "~1")),
                out,
            )
        }),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .for_each(|(i, c)| pointers(c, &format!("{here}/{i}"), out)),
        _ => {}
    }
}

/// One-step corruptions of a valid value: drop a member, null it, retype it, add an unknown field.
fn mutants(v: &Value) -> Vec<(String, Value)> {
    let mut ptrs = Vec::new();
    pointers(v, "", &mut ptrs);
    let mut out = Vec::new();
    for p in &ptrs {
        let node = v.pointer(p).unwrap();
        if node.is_object() {
            let mut m = v.clone();
            m.pointer_mut(p)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("zz_extra".into(), json!(1));
            out.push((format!("{p} + unknown field"), m));
        }
        if p.is_empty() {
            continue;
        }
        let (parent, last) = p.rsplit_once('/').unwrap();
        let mut dropped = v.clone();
        match dropped.pointer_mut(parent).unwrap() {
            Value::Object(m) => {
                m.remove(&last.replace("~1", "/").replace("~0", "~"));
            }
            Value::Array(a) => {
                a.remove(last.parse::<usize>().unwrap());
            }
            _ => unreachable!(),
        }
        out.push((format!("{p} dropped"), dropped));
        for (label, repl) in [
            ("null", Value::Null),
            ("number", json!(7)),
            ("string", json!("x")),
            ("true", json!(true)),
            ("empty array", json!([])),
        ] {
            let mut m = v.clone();
            *m.pointer_mut(p).unwrap() = repl;
            out.push((format!("{p} -> {label}"), m));
        }
    }
    out
}

/// Cross-field rules JSON Schema cannot state (the `rust_only` class of invalid examples), checked
/// here directly on the JSON rather than through the DTO: true when `value` breaks one of them.
/// Keyed by the def the Rust DTO is decoded as. Only DTOs that embed these types are listed:
/// a `Job` carries `duplicates` (`JobResult.job`, each `JobList.jobs` item). A `ResponseEnvelope`
/// `result` is an untyped payload that the envelope roundtrip does not decode, so it is not checked.
fn cross_field_refusal(def: &str, value: &Value) -> bool {
    match def {
        "TreeSlice" => parents_break_order(&value["nodes"]),
        "DuplicateReport" => report_dangles(value),
        "CollectionMembersUpdate" => add_and_remove_overlap(value),
        "JobResult" => report_in_job(&value["job"]),
        "JobList" => value["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(report_in_job),
        _ => false,
    }
}

fn report_in_job(job: &Value) -> bool {
    job["duplicates"].is_object() && report_dangles(&job["duplicates"])
}

/// SliceNode.parent is null only at index 0, and every other parent names an earlier node.
fn parents_break_order(nodes: &Value) -> bool {
    nodes
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .any(|(index, node)| {
            let parent = &node["parent"];
            if index == 0 {
                !parent.is_null()
            } else {
                parent.as_u64().is_none_or(|p| p >= index as u64)
            }
        })
}

/// Group, link and skipped indices refer to entries: each must be below `entries.len()`.
fn report_dangles(report: &Value) -> bool {
    let count = report["entries"].as_array().map_or(0, Vec::len) as u64;
    let dangling = |index: &Value| index.as_u64().is_none_or(|i| i >= count);
    let sets = |key: &str| {
        report[key]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|set| set.as_array().into_iter().flatten())
    };
    sets("groups").chain(sets("links")).any(dangling)
        || report["skipped"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|skip| dangling(&skip["entry"]))
}

/// A node may not be both added and removed in one update (semantics.md, Collections).
fn add_and_remove_overlap(update: &Value) -> bool {
    let members = |key: &str| update[key].as_array().cloned().unwrap_or_default();
    let remove = members("remove");
    members("add").iter().any(|id| remove.contains(id))
}

/// The Rust DTOs accept exactly what the schema accepts, less the cross-field refusals above, for
/// every one-step corruption of every example: a gap either way is a contract bug (a schema
/// loophole or a Rust loophole). A cross-field violation must be refused by Rust, never accepted.
#[test]
fn schema_and_dto_agree_on_corrupted_examples() {
    let mut schema = Schema::load();
    let mut checked = 0;
    let mut disagreements = Vec::new();
    for (label, def, value) in valid_examples() {
        for (what, mutant) in mutants(&value) {
            let schema_ok = schema.errors(&def, &mutant).is_empty();
            let rust_ok = roundtrip(&def, &mutant).unwrap().is_ok();
            let expected_rust_ok = schema_ok && !cross_field_refusal(&def, &mutant);
            checked += 1;
            if rust_ok != expected_rust_ok {
                disagreements.push(format!(
                    "{label} {what}: schema {schema_ok}, rust {rust_ok}"
                ));
            }
        }
    }
    assert!(checked > 5000, "only {checked} mutants");
    assert!(
        disagreements.is_empty(),
        "{} disagreements, first 25:\n{}",
        disagreements.len(),
        disagreements
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
