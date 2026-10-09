//! L1b: every example validates against its `$def` (JSON Schema) and round-trips through its
//! Rust DTO to an equal `serde_json::Value`; every negative fails in both; the schema keeps
//! its structural promises (closed objects, constant-false effects, no path-taking command).

mod common;

use common::registry::roundtrip;
use jsonschema::Validator;
use loomward_protocol::{Command, EventName};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

fn contracts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/v3")
}

fn read(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

struct Schema {
    root: Value,
    validators: HashMap<String, Validator>,
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
    for p in files("envelopes") {
        let doc = read(&p);
        out.push((
            stem(&p),
            doc["def"].as_str().unwrap().to_string(),
            doc["value"].clone(),
        ));
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
    assert!(
        files("invalid").len() >= 20,
        "the negatives are the point of the suite"
    );
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
    assert_eq!(examples.len(), 45 * 2 + 10 + files("envelopes").len());
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

#[test]
fn invalid_examples_fail_the_schema_and_the_dto() {
    let mut schema = Schema::load();
    for p in files("invalid") {
        let doc = read(&p);
        let (def, value) = (doc["def"].as_str().unwrap(), &doc["value"]);
        assert!(
            doc["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{} needs a reason",
            stem(&p)
        );
        assert!(
            !schema.errors(def, value).is_empty(),
            "{} passes the schema as {def}",
            stem(&p)
        );
        let rust = roundtrip(def, value).unwrap_or_else(|| panic!("no Rust type for {def}"));
        assert!(
            rust.is_err(),
            "{} deserialises as {def} but must not",
            stem(&p)
        );
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

/// The Rust DTOs accept exactly what the schema accepts, for every one-step corruption of
/// every example: a gap either way is a contract bug (a schema loophole or a Rust loophole).
#[test]
fn schema_and_dto_agree_on_corrupted_examples() {
    let mut schema = Schema::load();
    let mut checked = 0;
    let mut disagreements = Vec::new();
    for (label, def, value) in valid_examples() {
        for (what, mutant) in mutants(&value) {
            let schema_ok = schema.errors(&def, &mutant).is_empty();
            let rust_ok = roundtrip(&def, &mutant).unwrap().is_ok();
            checked += 1;
            if schema_ok != rust_ok {
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
