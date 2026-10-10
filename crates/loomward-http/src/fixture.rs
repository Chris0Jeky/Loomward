//! `FixtureService`: a stand-in [`ViewService`] that answers every command with its example result
//! from `contracts/v3/examples/commands/`, so the app's HTTP transport can run against a real
//! server before lane L8 lands the real service. It scans nothing, stores nothing and has no
//! effects; every response is the contract's synthetic example.
//!
//! Its event stream follows `semantics.md` section 7: `stream.hello` first and again after each
//! heartbeat interval of silence, replay from `Last-Event-ID` within the buffer, `stream.lagged`
//! for a replay gap, an unknown epoch, or a reader that fell behind the buffer. One buffer serves
//! as both the replay window and every subscriber's queue (capacity only, no 60 s age bound).

use loomward_protocol::limits::EVENT_QUEUE_CAPACITY;
use loomward_protocol::{
    CallContext, CloseReason, Command, DatasetClass, ErrorBody, ErrorCode, EventEnvelope,
    EventName, EventStream, Payload, RecvOutcome, RequestEnvelope, ResponseEnvelope, ResponseMeta,
    ViewService,
};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `contracts/v3/examples` of this checkout.
pub const EXAMPLES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../contracts/v3/examples");

/// Heartbeat of `semantics.md` section 7.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

pub struct FixtureService {
    dataset: DatasetClass,
    session_started_at: String,
    results: HashMap<Command, Payload>,
    bus: Arc<Bus>,
}

struct Bus {
    epoch: String,
    heartbeat: Duration,
    capacity: usize,
    state: Mutex<BusState>,
    wake: Condvar,
    open: AtomicUsize,
}

#[derive(Default)]
struct BusState {
    last_seq: u64,
    buffer: VecDeque<EventEnvelope>,
}

impl BusState {
    fn oldest(&self) -> u64 {
        self.buffer
            .front()
            .map_or(self.last_seq + 1, |e| e.seq.get())
    }
}

impl FixtureService {
    /// Loads every `<command>.result.json` from [`EXAMPLES_DIR`].
    pub fn new(dataset: DatasetClass) -> io::Result<Self> {
        Self::load(dataset, Path::new(EXAMPLES_DIR))
    }

    pub fn load(dataset: DatasetClass, examples: &Path) -> io::Result<Self> {
        let mut results = HashMap::new();
        for &command in Command::ALL {
            let file = examples
                .join("commands")
                .join(format!("{}.result.json", command.as_str()));
            let text = std::fs::read_to_string(&file)?;
            match serde_json::from_str(&text)? {
                Value::Object(map) => {
                    results.insert(command, map);
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} is not a JSON object", file.display()),
                    ))
                }
            }
        }
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(|e| io::Error::other(e.to_string()))?;
        let epoch = format!(
            "e_{}",
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        Ok(Self {
            dataset,
            session_started_at: now(),
            results,
            bus: Arc::new(Bus {
                epoch,
                heartbeat: HEARTBEAT,
                capacity: EVENT_QUEUE_CAPACITY,
                state: Mutex::new(BusState::default()),
                wake: Condvar::new(),
                open: AtomicUsize::new(0),
            }),
        })
    }

    /// Overrides the heartbeat interval and the buffer capacity (tests). Call before subscribing.
    pub fn with_stream_limits(mut self, heartbeat: Duration, capacity: usize) -> Self {
        let bus = Arc::get_mut(&mut self.bus).expect("no stream is open yet");
        bus.heartbeat = heartbeat;
        bus.capacity = capacity.max(1);
        self
    }

    pub fn epoch(&self) -> &str {
        &self.bus.epoch
    }

    /// Streams opened and not yet closed.
    pub fn open_streams(&self) -> usize {
        self.bus.open.load(Ordering::SeqCst)
    }

    /// Appends sequenced events atomically (one lock), as if the engine had committed them.
    pub fn publish(
        &self,
        events: impl IntoIterator<Item = (EventName, Payload)>,
    ) -> Result<(), ErrorBody> {
        let mut st = self.bus.lock();
        for (event, data) in events {
            let e = self
                .bus
                .envelope(st.last_seq + 1, event, Value::Object(data))?;
            st.last_seq += 1;
            st.buffer.push_back(e);
            while st.buffer.len() > self.bus.capacity {
                st.buffer.pop_front();
            }
        }
        drop(st);
        self.bus.wake.notify_all();
        Ok(())
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
        .expect("fixture meta is well formed")
    }
}

impl ViewService for FixtureService {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        let result = Command::from_name(&request.command).and_then(|c| {
            let mut r = self.results.get(&c)?.clone();
            if c == Command::SessionHello {
                r.insert("dataset_class".into(), json!(self.dataset));
                r.insert("adapter".into(), json!(ctx.adapter));
                r.insert("engine_version".into(), json!("fixture-service"));
                r.insert("session_started_at".into(), json!(self.session_started_at));
            }
            Some(r)
        });
        match result {
            Some(r) => ResponseEnvelope::ok_object(request.request_id, r, self.meta()),
            None => ResponseEnvelope::error(
                Some(request.request_id),
                ErrorBody::new(ErrorCode::UnknownCommand, "unknown command", false),
                None,
            ),
        }
    }

    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
        let bus = self.bus.clone();
        bus.open.fetch_add(1, Ordering::SeqCst);
        let st = bus.lock();
        let live = st.last_seq + 1;
        let mut pending = VecDeque::from([bus.hello(&st, self.dataset)]);
        let next = match resume {
            None => live,
            Some((epoch, seq)) if epoch == bus.epoch => {
                if seq >= st.last_seq {
                    live
                } else if seq + 1 >= st.oldest() {
                    seq + 1
                } else {
                    pending.push_back(bus.lagged(&st, "replay_gap", None));
                    live
                }
            }
            Some(_) => {
                pending.push_back(bus.lagged(&st, "epoch_changed", None));
                live
            }
        };
        drop(st);
        Box::new(FixtureStream {
            bus,
            dataset: self.dataset,
            next,
            pending,
            quiet_since: Instant::now(),
            closed: false,
        })
    }
}

impl Bus {
    fn lock(&self) -> MutexGuard<'_, BusState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn envelope(
        &self,
        seq: u64,
        event: EventName,
        data: Value,
    ) -> Result<EventEnvelope, ErrorBody> {
        let e: EventEnvelope = serde_json::from_value(json!({
            "protocol": "loomward/3",
            "epoch": self.epoch,
            "seq": seq,
            "event": event,
            "at": now(),
            "catalog_rev": null,
            "state_rev": null,
            "data": data,
        }))
        .map_err(|e| ErrorBody::invalid_request(&e.to_string()))?;
        e.validate()?;
        Ok(e)
    }

    /// Not sequenced: its envelope `seq` is the current `last_seq`.
    fn hello(&self, st: &BusState, dataset: DatasetClass) -> EventEnvelope {
        let data = json!({
            "session_started_at": now(),
            "epoch": self.epoch,
            "last_seq": st.last_seq,
            "oldest_replayable_seq": st.oldest(),
            "dataset_class": dataset,
        });
        self.envelope(st.last_seq, EventName::StreamHello, data)
            .expect("fixture hello is well formed")
    }

    fn lagged(&self, st: &BusState, reason: &str, dropped: Option<u64>) -> EventEnvelope {
        let data = json!({
            "reason": reason,
            "dropped": dropped,
            "resync": ["roots", "volumes", "jobs", "tree", "learning"],
        });
        self.envelope(st.last_seq, EventName::StreamLagged, data)
            .expect("fixture lagged is well formed")
    }
}

struct FixtureStream {
    bus: Arc<Bus>,
    dataset: DatasetClass,
    /// Next sequenced event this subscriber has not seen.
    next: u64,
    pending: VecDeque<EventEnvelope>,
    quiet_since: Instant,
    closed: bool,
}

impl EventStream for FixtureStream {
    fn epoch(&self) -> &str {
        &self.bus.epoch
    }

    fn recv_timeout(&mut self, timeout: Duration) -> RecvOutcome {
        if self.closed {
            return RecvOutcome::Closed(CloseReason::ClosedByAdapter);
        }
        if let Some(e) = self.pending.pop_front() {
            self.quiet_since = Instant::now();
            return RecvOutcome::Event(e);
        }
        let until = Instant::now() + timeout;
        let bus = self.bus.clone();
        let mut st = bus.lock();
        loop {
            let oldest = st.oldest();
            if self.next < oldest && !st.buffer.is_empty() {
                let dropped = oldest - self.next;
                self.next = oldest;
                self.quiet_since = Instant::now();
                return RecvOutcome::Event(bus.lagged(&st, "subscriber_overflow", Some(dropped)));
            }
            if let Some(e) = st.buffer.get((self.next - oldest) as usize) {
                self.next += 1;
                self.quiet_since = Instant::now();
                return RecvOutcome::Event(e.clone());
            }
            let now = Instant::now();
            let beat = self.quiet_since + bus.heartbeat;
            if now >= beat {
                self.quiet_since = now;
                return RecvOutcome::Event(bus.hello(&st, self.dataset));
            }
            if now >= until {
                return RecvOutcome::Timeout;
            }
            st = bus
                .wake
                .wait_timeout(st, beat.min(until) - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.bus.open.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for FixtureStream {
    fn drop(&mut self) {
        self.close();
    }
}

/// UTC now with milliseconds and a literal `Z` (`semantics.md` section 9).
fn now() -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use loomward_protocol::Timestamp;

    #[test]
    fn now_is_a_contract_timestamp() {
        assert!(Timestamp::new(now()).is_ok(), "{}", now());
    }

    #[test]
    fn every_fixture_answer_is_a_complete_envelope() {
        let service = FixtureService::new(DatasetClass::Personal).unwrap();
        for &command in Command::ALL {
            let request: RequestEnvelope = serde_json::from_value(json!({
                "protocol": "loomward/3", "request_id": "r_1", "command": command.as_str(), "payload": {}
            }))
            .unwrap();
            match service.call(request, &CallContext::http()) {
                ResponseEnvelope::Ok(ok) => {
                    ok.validate_for(command)
                        .unwrap_or_else(|e| panic!("{}: {e:?}", command.as_str()));
                    assert_eq!(ok.meta.dataset_class, DatasetClass::Personal);
                }
                ResponseEnvelope::Err(e) => panic!("{}: {e:?}", command.as_str()),
            }
        }
    }
}
