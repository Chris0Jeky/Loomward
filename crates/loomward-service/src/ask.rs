//! LW-118: the Ask commands (docs/46 section 8, `semantics.md` "Ask").
//!
//! The owner configures one loopback provider profile (desktop only), issues or revokes the
//! standing `ask:local` client grant, and asks. A turn runs on its own thread: the model's tools
//! go back through [`ViewService::call`] as the `ask:local` client, so the LW-067 grant is applied
//! by the service on every call and a revocation holds from the next one. Nothing here has an
//! effect: the answer is text, and the only writes are the owner's profile (`st.meta`) and grant.

use crate::{fail, now, now_ns, sql_error, ts_ns, with_reason, Inner, Out, Service};
use loomward_ask::turn::{self, Answer, OutboundRecord};
use loomward_ask::{ErrorKind, Provider, ProviderClass, ProviderError, ProviderProfile};
use loomward_protocol::{
    AskConfigureRequest, AskGrantRequest, AskTurnRef, AskTurnRequest, CallContext, ErrorBody,
    ErrorCode, EventStream, RequestEnvelope, ResponseEnvelope, ViewService,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, Weak};

type Handled = Result<Out, ErrorBody>;

/// The only Ask principal that runs: the cloud class waits on q-5.
pub(crate) const PRINCIPAL: &str = "ask:local";
const ANSWER_CHARS: usize = 16_000;
const LIST_CAP: usize = 64;

/// Opens a provider for a stored profile. The default spawns the broker beside this executable.
pub(crate) type Factory =
    Arc<dyn Fn(&ProviderProfile) -> Result<Box<dyn Provider>, ProviderError> + Send + Sync>;

pub(crate) struct Asks {
    /// The latest turn of this session (running or settled), as its contract JSON.
    slot: Mutex<Option<Value>>,
    pub(crate) factory: Mutex<Factory>,
}

impl Default for Asks {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
            factory: Mutex::new(Arc::new(broker)),
        }
    }
}

fn broker(profile: &ProviderProfile) -> Result<Box<dyn Provider>, ProviderError> {
    let name = format!("loomward-broker{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .ok_or_else(|| {
            ProviderError::new(ErrorKind::BrokerFailed, "the broker was not found", false)
        })?;
    loomward_ask::open(profile, exe, None).map(|b| Box::new(b) as Box<dyn Provider>)
}

/// The service as the turn's tools see it: a weak handle, so a turn never keeps a closed service
/// alive between calls, and a `Service` that does not shut the engine down when dropped.
struct Peer(Weak<Inner>);

impl Peer {
    fn service(&self) -> Option<Service> {
        self.0.upgrade().map(|inner| Service {
            inner: crate::OwnedInner(Some(inner)),
            peer: true,
        })
    }
}

impl ViewService for Peer {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        match self.service() {
            Some(s) => s.call(request, ctx),
            None => ResponseEnvelope::error(
                Some(request.request_id),
                fail(ErrorCode::Cancelled, "the service closed"),
                None,
            ),
        }
    }

    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
        self.service()
            .expect("a turn never subscribes after the service closed")
            .subscribe(resume)
    }
}

fn status_out(v: Value) -> Handled {
    Ok(Out {
        result: v,
        catalog_rev: None,
        state_rev: None,
    })
}

fn kind_name(kind: ErrorKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

impl Inner {
    fn ask_profile(&self) -> Result<Option<ProviderProfile>, ErrorBody> {
        let json: Option<String> = self
            .db()
            .query_row(
                "SELECT value FROM st.meta WHERE key='ask_profile'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        // A record that no longer reads is no profile: nothing is contacted.
        Ok(json.and_then(|j| {
            let v: Value = serde_json::from_str(&j).ok()?;
            Some(ProviderProfile {
                base_url: v.get("base_url")?.as_str()?.into(),
                model: v.get("model")?.as_str()?.into(),
            })
        }))
    }

    pub(crate) fn ask_status(&self) -> Handled {
        let profile = self.ask_profile()?;
        let grant = match self.client_grant(PRINCIPAL) {
            Ok(g) => {
                let at: i64 = self
                    .db()
                    .query_row(
                        "SELECT created_at_ns FROM st.client_grant WHERE id=?1",
                        [g.epoch],
                        |r| r.get(0),
                    )
                    .map_err(sql_error)?;
                let roots: Vec<String> = g.roots.iter().map(|r| self.root_id(*r)).collect();
                json!({"roots": roots, "fields": g.fields, "granted_at": ts_ns(at)})
            }
            Err(e) if e.code == ErrorCode::PermissionDenied => Value::Null,
            Err(e) => return Err(e),
        };
        let tools: Vec<Value> = turn::TOOLS
            .iter()
            .map(|(name, by)| json!({"name": name, "backed_by": by, "read_only": true}))
            .collect();
        let last = self
            .ask
            .slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|t| t["turn_id"].clone());
        status_out(json!({
            "provider": if profile.is_some() { "local" } else { "not_configured" },
            "profile_digest": profile.as_ref().map(ProviderProfile::digest),
            "profile": profile.map(|p| json!({"base_url": p.base_url, "model": p.model})),
            "principal": PRINCIPAL,
            "grant": grant,
            "tools": tools,
            "last_turn": last,
        }))
    }

    /// Desktop only (the command table keeps it off HTTP). A cloud-class profile is refused.
    pub(crate) fn ask_configure(&self, r: AskConfigureRequest) -> Handled {
        let profile = r.profile.map(|p| ProviderProfile {
            base_url: p.base_url.as_str().into(),
            model: p.model.as_str().into(),
        });
        if profile
            .as_ref()
            .is_some_and(|p| p.class() != ProviderClass::Local)
        {
            return Err(with_reason(
                fail(
                    ErrorCode::CapabilityUnavailable,
                    "only a loopback endpoint with a local model can be used; cloud providers wait on q-5",
                ),
                "ask_cloud_gated",
            ));
        }
        {
            let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
            let conn = self.db();
            match &profile {
                Some(p) => conn.execute(
                    "INSERT INTO st.meta(key,value) VALUES('ask_profile',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    [json!({"base_url": p.base_url, "model": p.model}).to_string()],
                ),
                None => conn.execute("DELETE FROM st.meta WHERE key='ask_profile'", []),
            }
            .map_err(sql_error)?;
        }
        self.ask_status()
    }

    pub(crate) fn ask_grant(&self, r: AskGrantRequest) -> Handled {
        let fields: Vec<String> = r
            .fields
            .iter()
            .filter_map(|f| serde_json::to_value(f).ok()?.as_str().map(str::to_owned))
            .collect();
        let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
        self.grant_client(PRINCIPAL, &r.roots, &fields, None)?;
        self.ask_status()
    }

    pub(crate) fn ask_revoke(&self) -> Handled {
        self.revoke_client(PRINCIPAL)?;
        self.ask_status()
    }

    pub(crate) fn ask_result(&self, r: AskTurnRef) -> Handled {
        let slot = self.ask.slot.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            Some(t) if t["turn_id"] == r.turn_id.as_str() => status_out(t.clone()),
            _ => Err(fail(ErrorCode::NotFound, "unknown turn")),
        }
    }

    /// Starts a turn and answers with it running. Refused before anything is sent without a
    /// profile or a live grant over at least one root; one turn at a time.
    pub(crate) fn ask_turn(&self, r: AskTurnRequest) -> Handled {
        let Some(profile) = self.ask_profile()? else {
            return Err(with_reason(
                fail(
                    ErrorCode::CapabilityUnavailable,
                    "no model is configured: add a local model endpoint in the desktop app",
                ),
                "ask_not_configured",
            ));
        };
        if !self
            .client_grant(PRINCIPAL)
            .is_ok_and(|g| !g.roots.is_empty())
        {
            return Err(with_reason(
                fail(
                    ErrorCode::PermissionDenied,
                    "Ask may see no folder: grant it at least one folder first",
                ),
                "ask_no_grant",
            ));
        }
        let mut slot = self.ask.slot.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|t| t["state"] == "running") {
            return Err(fail(ErrorCode::Busy, "a question is still being answered"));
        }
        let question = r.question.as_str().to_owned();
        let running = json!({
            "turn_id": format!("at_{}", now_ns()), "state": "running", "question": question,
            "started_at": now(), "finished_at": null, "answer": null, "answer_truncated": false,
            "failure": null, "citations": [], "unverified": [], "tool_calls": [],
            "tool_calls_total": 0, "rounds": [],
        });
        let factory = self
            .ask
            .factory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let weak = self.weak.clone();
        let base = running.clone();
        std::thread::Builder::new()
            .name("lw-ask".into())
            .spawn(move || {
                let view = Peer(weak.clone());
                let mut log = Vec::new();
                let result = factory(&profile)
                    .and_then(|p| turn::ask(&view, &*p, &profile, &question, &mut log));
                if let Some(inner) = weak.upgrade() {
                    let done = settle(&view, base, result, &log);
                    let mut slot = inner.ask.slot.lock().unwrap_or_else(|e| e.into_inner());
                    if slot
                        .as_ref()
                        .is_some_and(|t| t["turn_id"] == done["turn_id"])
                    {
                        *slot = Some(done);
                    }
                }
            })
            .map_err(|_| fail(ErrorCode::ResourceBudget, "no worker"))?;
        *slot = Some(running.clone());
        status_out(running)
    }
}

/// The settled record: the answer, citations resolved from the ledger (as the owner, so a chip
/// says whether its entry still reads), unverified handles, the tool log and every round's record.
fn settle(
    view: &Peer,
    mut t: Value,
    result: Result<Answer, ProviderError>,
    log: &[OutboundRecord],
) -> Value {
    t["finished_at"] = json!(now());
    t["rounds"] = log
        .iter()
        .map(|r| {
            json!({"round": r.round, "profile_digest": r.profile_digest, "principal": r.principal,
                   "sha256": r.sha256, "bytes": r.bytes, "dispatched": r.dispatched,
                   "outcome": cut(&r.outcome, 32)})
        })
        .collect();
    let answer = match result {
        Ok(a) => a,
        Err(e) => {
            t["state"] = json!("failed");
            t["failure"] = json!({"kind": kind_name(e.kind), "message": cut(&e.message, 256),
                                  "dispatched": e.dispatched});
            return t;
        }
    };
    t["state"] = json!("completed");
    let shown = cut(&answer.text, ANSWER_CHARS);
    t["answer_truncated"] = json!(shown.len() < answer.text.len());
    let (mut citations, mut unverified) = (Vec::new(), Vec::new());
    for h in turn::cited(&shown) {
        match answer.handles.iter().find(|(k, _)| *k == h) {
            // Bounded by the ledger (turn::MAX_LEDGER), so no cited row is ever mislabelled.
            Some((_, node)) => citations.push(cite(view, &h, node)),
            None if unverified.len() < LIST_CAP => unverified.push(h),
            _ => {}
        }
    }
    t["answer"] = json!(shown);
    t["citations"] = json!(citations);
    t["unverified"] = json!(unverified);
    t["tool_calls_total"] = json!(answer.tools.len());
    t["tool_calls"] = answer
        .tools
        .iter()
        .take(LIST_CAP)
        .map(|u| json!({"name": u.name, "ok": u.ok, "rows": u.rows, "detail": u.detail}))
        .collect();
    t
}

fn cite(view: &Peer, handle: &str, node: &str) -> Value {
    let req = serde_json::from_value(json!({
        "protocol": "loomward/3", "request_id": "ask_cite", "command": "node.inspect",
        "payload": {"node_id": node}
    }))
    .expect("a valid request");
    let (name, root) = match view.call(req, &CallContext::http()) {
        ResponseEnvelope::Ok(ok) => (
            ok.result["row"]["name"].as_str().map(|n| cut(n, 255)),
            ok.result.get("root_id").cloned().unwrap_or(Value::Null),
        ),
        ResponseEnvelope::Err(_) => (None, Value::Null),
    };
    json!({"handle": handle, "node_id": node, "resolved": name.is_some(), "name": name, "root_id": root})
}

#[cfg(test)]
#[path = "ask_tests.rs"]
mod tests;
