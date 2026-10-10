//! The desktop adapter (docs/41 sections 5.1 and 12, ADR-V3-03): exactly two IPC commands,
//! `lw_call` and `lw_events`, between the Tauri webview and the [`ViewService`].
//!
//! This module holds the command layer without the Tauri runtime, so tests and
//! `loomward-desktop --self-test` drive the same code the webview reaches. `main.rs` only binds
//! it to `#[tauri::command]`s, a `Channel` sink and the window/page lifecycle.
//!
//! Nothing here accepts a path, runs anything outside the protocol or has an effect (AGENTS.md
//! invariant 1). Native dialogs are reachable only through the service's [`CallContext`], never
//! from the webview directly ([`dialogs`]).

pub mod dialogs;
pub mod selftest;
pub mod shell;

use loomward_protocol::limits::MAX_REQUEST_BYTES;
use loomward_protocol::{
    Adapter, CallContext, CloseReason, Command, DatasetClass, ErrorBody, ErrorCode, EventEnvelope,
    NativeDialogs, RecvOutcome, RequestEnvelope, ResponseEnvelope, ResponseMeta, StreamEpoch,
    ViewService,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Event streams one webview may hold open at once, as for HTTP (`semantics.md` section 7).
pub const MAX_STREAMS_PER_WEBVIEW: usize = 4;
/// How long the first event (`stream.hello`) may take before `lw_events` fails.
const FIRST_EVENT_TIMEOUT: Duration = Duration::from_secs(5);
/// How often an idle pump rechecks for teardown.
pub const PUMP_POLL: Duration = Duration::from_millis(250);

/// One open stream's teardown flag and liveness.
#[derive(Default)]
struct Slot {
    stop: AtomicBool,
    done: AtomicBool,
}

pub struct Desktop {
    service: Arc<dyn ViewService>,
    dataset: DatasetClass,
    dialogs: Arc<dyn NativeDialogs>,
    /// Open streams by webview label.
    streams: Mutex<HashMap<String, Vec<Arc<Slot>>>>,
}

impl Desktop {
    pub fn new(
        service: Arc<dyn ViewService>,
        dataset: DatasetClass,
        dialogs: Arc<dyn NativeDialogs>,
    ) -> Self {
        Self {
            service,
            dataset,
            dialogs,
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// `lw_call`. `Err` is a transport fault (the invoke promise rejects); every protocol outcome,
    /// refusals included, is an `Ok` envelope. The request enters only through
    /// [`RequestEnvelope::from_slice`], as over HTTP.
    pub fn call(&self, request: &Value) -> Result<ResponseEnvelope, String> {
        let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err("the request exceeds 64 KiB".into());
        }
        let request = match RequestEnvelope::from_slice(&bytes) {
            Ok(r) => r,
            Err(rejected) => return Ok(rejected.into_response()),
        };
        let command = Command::from_name(&request.command);
        if !command.is_some_and(|c| c.available_on(Adapter::Tauri)) {
            let error = ErrorBody::new(
                ErrorCode::CapabilityUnavailable,
                "this command is not offered by the desktop adapter",
                false,
            );
            return Ok(ResponseEnvelope::error(
                Some(request.request_id),
                error,
                None,
            ));
        }
        // Fail closed before the service, so neither dialog can open for these (docs/41 5.2,
        // semantics.md 11). ponytail: the real service (lane L8) must refuse these too; this guard
        // stays as the adapter's second wall and because the fixture would answer "granted".
        let refusal = match (command, self.dataset) {
            (Some(Command::RootsRequestGrant), DatasetClass::Synthetic) => Some(json!({
                "outcome": "refused", "root": null, "refusal": "synthetic_session_requires_lab_root"
            })),
            (Some(Command::GrantsCreateDisclosure), DatasetClass::Personal) => Some(json!({
                "outcome": "refused", "grant": null, "refusal": "confinement_not_enforced"
            })),
            _ => None,
        };
        if let Some(Value::Object(result)) = refusal {
            return Ok(ResponseEnvelope::ok_object(
                request.request_id,
                result,
                self.meta(),
            ));
        }
        Ok(self
            .service
            .call(request, &CallContext::tauri(self.dialogs.clone())))
    }

    /// `lw_events`. Opens a stream for `webview`, delivers `stream.hello` (and any replay or
    /// `stream.lagged`) through `sink`, then pumps on a thread until the sink fails, the service
    /// closes the stream or [`Desktop::close_webview`] runs. `sink` returns `false` once the
    /// webview can no longer receive.
    pub fn open_stream(
        &self,
        webview: &str,
        last_epoch: Option<String>,
        last_seq: Option<u64>,
        mut sink: impl FnMut(EventEnvelope) -> bool + Send + 'static,
    ) -> Result<(), String> {
        let slot = Arc::new(Slot::default());
        {
            let mut streams = self.lock();
            let open = streams.entry(webview.to_string()).or_default();
            open.retain(|s| !s.done.load(Ordering::SeqCst));
            if open.len() >= MAX_STREAMS_PER_WEBVIEW {
                return Err("at most four event streams per window".into());
            }
            open.push(slot.clone());
        }
        let release = |slot: &Slot| slot.done.store(true, Ordering::SeqCst);
        let mut stream = self.service.subscribe(resume(last_epoch, last_seq));
        let first = match stream.recv_timeout(FIRST_EVENT_TIMEOUT) {
            RecvOutcome::Event(e) => e,
            RecvOutcome::Closed(CloseReason::TooManyStreams) => {
                stream.close();
                release(&slot);
                return Err("at most four event streams".into());
            }
            _ => {
                stream.close();
                release(&slot);
                return Err("the event stream did not open".into());
            }
        };
        if !sink(first) {
            stream.close();
            release(&slot);
            return Err("the window cannot receive events".into());
        }
        let pump_slot = slot.clone();
        let spawned = std::thread::Builder::new()
            .name("loomward-channel".into())
            .spawn(move || {
                while !pump_slot.stop.load(Ordering::SeqCst) {
                    match stream.recv_timeout(PUMP_POLL) {
                        RecvOutcome::Event(e) => {
                            if !sink(e) {
                                break;
                            }
                        }
                        RecvOutcome::Timeout => {}
                        RecvOutcome::Closed(_) => break,
                    }
                }
                stream.close();
                pump_slot.done.store(true, Ordering::SeqCst);
            });
        if spawned.is_err() {
            release(&slot);
            return Err("the event stream did not open".into());
        }
        Ok(())
    }

    /// Tears down every stream of `webview`: on window close and on every page load (a reloaded
    /// page can never receive on its old channels). Each pump stops within [`PUMP_POLL`].
    /// Returns how many were still running.
    pub fn close_webview(&self, webview: &str) -> usize {
        let open = self.lock().remove(webview).unwrap_or_default();
        open.iter()
            .filter(|s| !s.stop.swap(true, Ordering::SeqCst) && !s.done.load(Ordering::SeqCst))
            .count()
    }

    /// Streams of `webview` whose pump is still running.
    pub fn open_streams(&self, webview: &str) -> usize {
        self.lock().get(webview).map_or(0, |v| {
            v.iter().filter(|s| !s.done.load(Ordering::SeqCst)).count()
        })
    }

    pub fn dataset(&self) -> DatasetClass {
        self.dataset
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Arc<Slot>>>> {
        self.streams.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn meta(&self) -> ResponseMeta {
        serde_json::from_value(json!({
            "served_at": now(),
            "elapsed_ms": 0,
            "dataset_class": self.dataset,
            "budget_hit": false,
            "catalog_rev": null,
            "state_rev": null,
        }))
        .expect("adapter meta is well formed")
    }
}

/// `lastEpoch` / `lastSeq` to the service's resume point. An epoch that is not a valid
/// `StreamEpoch`, or a `lastSeq` without an epoch, resumes from an unknown epoch, so the service
/// answers `stream.lagged { reason: "epoch_changed" }` and the client resyncs (as HTTP does for a
/// malformed `Last-Event-ID`).
pub fn resume(last_epoch: Option<String>, last_seq: Option<u64>) -> Option<(String, u64)> {
    match (last_epoch, last_seq) {
        (None, None) => None,
        (epoch, seq) => {
            let epoch = epoch
                .and_then(|e| StreamEpoch::new(e).ok())
                .map(String::from)
                .unwrap_or_default();
            Some((epoch, seq.unwrap_or(0)))
        }
    }
}

/// UTC now with milliseconds and a literal `Z` (`semantics.md` section 9).
// ponytail: copy of loomward_http::fixture's private `now`; share it when L8 adds a clock module.
pub fn now() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}
