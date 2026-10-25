use crate::cli::Args;
use loomward_protocol::{CallContext, RequestEnvelope, ResponseEnvelope, ViewService};
use loomward_service::{paths::lock_state_dir, ClientViewStamp, Service};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::fs::File;
use std::time::{Duration, Instant};

const UNKNOWN: &str = "unknown reference; call find again";
const MAX_REFERENCES: usize = 4096;

struct Tool {
    name: &'static str,
    command: &'static str,
    argument: &'static str,
    max_length: usize,
    description: &'static str,
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "find",
        command: "search.query",
        argument: "text",
        max_length: 256,
        description: "Find at most 20 metadata rows in the owner's grant. No scan or file read.",
    },
    Tool {
        name: "describe",
        command: "node.inspect",
        argument: "reference",
        max_length: 32,
        description: "Describe a reference returned by find in this service session.",
    },
];

pub fn known(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}

pub fn specs() -> Value {
    Value::Array(TOOLS.iter().map(|tool| json!({
        "name":tool.name, "description":tool.description,
        "inputSchema":{"type":"object","properties":{
            (tool.argument):{"type":"string","maxLength":tool.max_length}
        },"required":[tool.argument],"additionalProperties":false},
        "annotations":{"readOnlyHint":true,"destructiveHint":false,"openWorldHint":false}
    })).collect())
}

// Field order guarantees that shutdown completes before the OS lock is released.
struct Locked {
    service: Service,
    _lock: File,
}

pub struct Host {
    args: Args,
    loaded: Option<Locked>,
    last_call: Instant,
    references: VecDeque<(String, String)>,
    stamp: Option<ClientViewStamp>,
    // Never reset on release: an old r1 must not alias a new service's first result.
    next_reference: u64,
}

impl Host {
    pub fn new(args: Args) -> Self {
        Self {
            args,
            loaded: None,
            last_call: Instant::now(),
            references: VecDeque::new(),
            stamp: None,
            next_reference: 0,
        }
    }

    pub fn time_left(&self) -> Option<Duration> {
        (self.loaded.is_some() && !self.args.idle.is_zero())
            .then(|| self.args.idle.saturating_sub(self.last_call.elapsed()))
    }

    pub fn release_if_idle(&mut self) {
        if self.time_left().is_some_and(|left| left.is_zero()) {
            self.references.clear();
            self.stamp = None;
            self.loaded.take();
        }
    }

    fn open(&mut self) -> Result<(), String> {
        if self.loaded.is_none() {
            let lock = lock_state_dir(&self.args.config.state_dir).map_err(|error| {
                if error.starts_with("busy:") {
                    "busy: Loomward is open on this machine; try again after it closes".into()
                } else {
                    eprintln!("loomward-mcp: {error}");
                    "cannot lock Loomward state".to_string()
                }
            })?;
            let service = Service::open_client(self.args.config.clone()).map_err(|error| {
                eprintln!("loomward-mcp: {error}");
                "cannot open Loomward state".to_string()
            })?;
            self.loaded = Some(Locked {
                service,
                _lock: lock,
            });
        }
        Ok(())
    }

    pub fn run(&mut self, name: &str, args: &Map<String, Value>) -> Result<Value, String> {
        let tool = TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .ok_or("unknown tool")?;
        let text = args
            .get(tool.argument)
            .and_then(Value::as_str)
            .filter(|s| s.chars().count() <= tool.max_length)
            .filter(|_| args.len() == 1)
            .ok_or_else(|| {
                format!(
                    "expected only `{}`: a string of at most {} characters",
                    tool.argument, tool.max_length
                )
            })?;
        self.open()?;
        self.last_call = Instant::now();
        let stamp = match self
            .loaded
            .as_ref()
            .expect("opened")
            .service
            .client_view_stamp(&self.args.principal)
        {
            Ok(stamp) => stamp,
            Err(error) => {
                self.references.clear();
                return Err(service_error(error));
            }
        };
        if self
            .stamp
            .as_ref()
            .is_some_and(|previous| previous != &stamp)
        {
            self.references.clear();
        }
        self.stamp = Some(stamp);
        let node = if name == "describe" {
            Some(
                self.references
                    .iter()
                    .find(|(r, _)| r == text)
                    .map(|(_, n)| n.clone())
                    .ok_or(UNKNOWN)?,
            )
        } else {
            None
        };
        let payload = match node {
            Some(node) => json!({"node_id":node}),
            None => json!({"root_id":null,"text":text,"extension":null,"min_bytes":null,
                "max_bytes":null,"modified_after":null,"modified_before":null,
                "kind":"any","limit":20,"cursor":null}),
        };
        let request: RequestEnvelope = serde_json::from_value(json!({
            "protocol":"loomward/3","request_id":"mcp_read","command":tool.command,"payload":payload
        }))
        .map_err(|_| "invalid service request")?;
        let response = self
            .loaded
            .as_ref()
            .expect("opened")
            .service
            .call(request, &CallContext::client(&self.args.principal));
        self.last_call = Instant::now();
        let result = match response {
            ResponseEnvelope::Ok(ok) => Value::Object(ok.result),
            ResponseEnvelope::Err(error) => {
                // A failed grant/scope check also invalidates references already disclosed.
                self.references.clear();
                return Err(service_error(error.error));
            }
        };
        let after = self
            .loaded
            .as_ref()
            .expect("opened")
            .service
            .client_view_stamp(&self.args.principal)
            .map_err(|error| {
                self.references.clear();
                service_error(error)
            })?;
        if self.stamp.as_ref() != Some(&after) {
            self.references.clear();
            return Err(UNKNOWN.into());
        }
        if name == "describe" {
            return self.row(&result);
        }
        let hits = result
            .get("hits")
            .and_then(Value::as_array)
            .ok_or("invalid service result")?;
        let rows = hits
            .iter()
            .take(20)
            .map(|hit| self.row(hit))
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Map::new();
        for key in ["partial", "index_state", "budget_hit", "substring_searched"] {
            if let Some(v) = result.get(key) {
                out.insert(key.into(), v.clone());
            }
        }
        let roots: Vec<_> = result
            .get("roots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|r| {
                let mut out = Map::new();
                for key in ["scan_state", "coverage"] {
                    if let Some(v) = r.get(key) {
                        out.insert(key.into(), v.clone());
                    }
                }
                Value::Object(out)
            })
            .collect();
        out.insert("roots".into(), roots.into());
        out.insert("rows".into(), rows.into());
        out.insert(
            "more".into(),
            json!(hits.len() > 20 || result.get("next_cursor").is_some_and(|v| !v.is_null())),
        );
        Ok(out.into())
    }

    fn row(&mut self, item: &Value) -> Result<Value, String> {
        let mut row = item
            .get("row")
            .and_then(Value::as_object)
            .cloned()
            .ok_or("invalid service row")?;
        let node = row
            .remove("node_id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or("invalid service node")?;
        let reference = match self.references.iter().find(|(_, n)| n == &node) {
            Some((r, _)) => r.clone(),
            None => {
                self.next_reference = self
                    .next_reference
                    .checked_add(1)
                    .ok_or("reference budget spent")?;
                let r = format!("r{}", self.next_reference);
                if self.references.len() == MAX_REFERENCES {
                    self.references.pop_front();
                }
                self.references.push_back((r.clone(), node));
                r
            }
        };
        let mut out = Map::new();
        out.insert("reference".into(), reference.into());
        out.insert("row".into(), row.into());
        for key in ["match_kind", "freshness", "path", "display_path"] {
            if let Some(v) = item.get(key) {
                out.insert(key.into(), v.clone());
            }
        }
        Ok(out.into())
    }
}

fn service_error(error: loomward_protocol::ErrorBody) -> String {
    let code = serde_json::to_value(error.code).unwrap_or(Value::Null);
    format!("{}: {}", code.as_str().unwrap_or("error"), &*error.message)
}
