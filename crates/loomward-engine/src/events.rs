//! Epoch-scoped event replay with bounded slow-reader state and silence heartbeats.
use crate::{Engine, EngineError, EngineResult};
use loomward_protocol::service::EVENT_QUEUE_CAPACITY;
use loomward_protocol::{
    CloseReason, Count, DatasetClass, EventEnvelope, EventName, EventStream, Generation, Payload,
    Protocol, RecvOutcome, StreamEpoch, Timestamp,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Position from Last-Event-ID; sequences compare only inside the same epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventResume {
    /// Service epoch.
    pub epoch: String,
    /// Last applied sequence.
    pub last_seq: u64,
}
impl EventResume {
    /// Parse the SSE `epoch.seq` spelling without accepting an invalid epoch or count.
    pub fn from_last_event_id(id: &str) -> Option<Self> {
        let (epoch, seq) = id.rsplit_once('.')?;
        StreamEpoch::new(epoch).ok()?;
        let last_seq = seq.parse::<u64>().ok()?;
        Count::new(last_seq).ok()?;
        Some(Self {
            epoch: epoch.into(),
            last_seq,
        })
    }
}
pub(crate) fn timestamp() -> Timestamp {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days = (seconds / 86400) as i64;
    // Gregorian civil date from days since Unix epoch.
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(m <= 2);
    Timestamp::new(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        seconds % 86400 / 3600,
        seconds % 3600 / 60,
        seconds % 60
    ))
    .unwrap()
}
pub(crate) fn random_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS random source");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}{hex}")
}
#[derive(Debug)]
struct Retained {
    event: EventEnvelope,
    published: Instant,
}

#[derive(Debug)]
struct State {
    seq: u64,
    replay: VecDeque<Retained>,
}
#[derive(Debug)]
pub(crate) struct Bus {
    epoch: String,
    started: Timestamp,
    class: DatasetClass,
    interval: Duration,
    state: Mutex<State>,
    changed: Condvar,
}
impl Bus {
    pub fn new(class: DatasetClass, interval: Duration) -> Self {
        Self {
            epoch: random_id("e_"),
            started: timestamp(),
            class,
            interval,
            state: Mutex::new(State {
                seq: 0,
                replay: VecDeque::new(),
            }),
            changed: Condvar::new(),
        }
    }
    fn oldest_replayable(&self, state: &State) -> u64 {
        state
            .replay
            .iter()
            .find(|e| e.published.elapsed() < Duration::from_secs(60))
            .map_or(state.seq + 1, |e| e.event.seq.get())
    }
    fn envelope(
        &self,
        state: &State,
        name: EventName,
        data: Payload,
        catalog_rev: Option<Generation>,
        state_rev: Option<Generation>,
    ) -> EventEnvelope {
        EventEnvelope {
            protocol: Protocol,
            epoch: StreamEpoch::new(&self.epoch).unwrap(),
            seq: Count::new(state.seq).unwrap(),
            event: name,
            at: timestamp(),
            catalog_rev,
            state_rev,
            data,
        }
    }
    fn hello(&self, state: &State) -> EventEnvelope {
        let data = serde_json::json!({"session_started_at":self.started,"epoch":self.epoch,"last_seq":state.seq,"oldest_replayable_seq":self.oldest_replayable(state),"dataset_class":self.class});
        self.envelope(
            state,
            EventName::StreamHello,
            data.as_object().unwrap().clone(),
            None,
            None,
        )
    }
    fn lagged(&self, state: &State, reason: &str, dropped: Option<u64>) -> EventEnvelope {
        self.envelope(state,EventName::StreamLagged,serde_json::json!({"reason":reason,"dropped":dropped,"resync":["roots","volumes","jobs","tree","learning"]}).as_object().unwrap().clone(),None,None)
    }
    pub fn publish(
        &self,
        name: EventName,
        data: Payload,
        catalog_rev: Option<Generation>,
        state_rev: Option<Generation>,
    ) -> EngineResult<()> {
        if matches!(name, EventName::StreamHello | EventName::StreamLagged) {
            return Err(EngineError::InvalidRequest {
                message: "stream controls are subscription-owned".into(),
            });
        }
        name.validate_data(&data)
            .map_err(|_| EngineError::InvalidRequest {
                message: "invalid event payload".into(),
            })?;
        let mut state = self.state.lock().unwrap();
        state.seq = state
            .seq
            .checked_add(1)
            .filter(|n| Count::new(*n).is_ok())
            .ok_or_else(|| EngineError::Internal {
                message: "event sequence exhausted".into(),
                detail: None,
            })?;
        let event = self.envelope(&state, name, data, catalog_rev, state_rev);
        state.replay.push_back(Retained {
            event,
            published: Instant::now(),
        });
        if state.replay.len() > EVENT_QUEUE_CAPACITY {
            state.replay.pop_front();
        }
        self.changed.notify_all();
        Ok(())
    }
    fn subscribe(self: &Arc<Self>, resume: Option<EventResume>) -> Subscription {
        let state = self.state.lock().unwrap();
        let mut initial = VecDeque::from([self.hello(&state)]);
        let mut cursor = state.seq;
        if let Some(r) = resume {
            let oldest = self.oldest_replayable(&state);
            if r.epoch != self.epoch {
                initial.push_back(self.lagged(&state, "epoch_changed", None));
            } else if r.last_seq > state.seq || r.last_seq.saturating_add(1) < oldest {
                initial.push_back(self.lagged(&state, "replay_gap", None));
            } else {
                cursor = r.last_seq;
            }
        }
        Subscription {
            bus: self.clone(),
            epoch: self.epoch.clone(),
            cursor,
            initial,
            closed: false,
            last_delivery: Instant::now(),
        }
    }
}
struct Subscription {
    bus: Arc<Bus>,
    epoch: String,
    cursor: u64,
    initial: VecDeque<EventEnvelope>,
    closed: bool,
    last_delivery: Instant,
}
impl EventStream for Subscription {
    fn epoch(&self) -> &str {
        &self.epoch
    }
    fn close(&mut self) {
        self.closed = true;
        self.initial.clear();
    }
    fn recv_timeout(&mut self, timeout: Duration) -> RecvOutcome {
        if self.closed {
            return RecvOutcome::Closed(CloseReason::ClosedByAdapter);
        }
        if let Some(e) = self.initial.pop_front() {
            self.last_delivery = Instant::now();
            return RecvOutcome::Event(e);
        }
        let deadline = Instant::now() + timeout;
        let mut state = self.bus.state.lock().unwrap();
        loop {
            let oldest = state
                .replay
                .front()
                .map_or(state.seq + 1, |e| e.event.seq.get());
            if self.cursor.saturating_add(1) < oldest {
                let dropped = state.seq - self.cursor;
                self.cursor = state.seq;
                self.last_delivery = Instant::now();
                return RecvOutcome::Event(self.bus.lagged(
                    &state,
                    "subscriber_overflow",
                    Some(dropped),
                ));
            }
            if let Some(e) = state
                .replay
                .iter()
                .find(|e| e.event.seq.get() > self.cursor)
            {
                self.cursor = e.event.seq.get();
                self.last_delivery = Instant::now();
                return RecvOutcome::Event(e.event.clone());
            }
            let silence = self.last_delivery.elapsed();
            if silence >= self.bus.interval {
                self.last_delivery = Instant::now();
                return RecvOutcome::Event(self.bus.hello(&state));
            }
            let now = Instant::now();
            if now >= deadline {
                return RecvOutcome::Timeout;
            }
            state = self
                .bus
                .changed
                .wait_timeout(state, (deadline - now).min(self.bus.interval - silence))
                .unwrap()
                .0;
        }
    }
}
impl Engine {
    /// Open an event subscription; first hello is followed by replay or a bounded lag notice.
    pub fn subscribe_events(
        &self,
        resume: Option<EventResume>,
    ) -> EngineResult<Box<dyn EventStream>> {
        Ok(Box::new(self.events.subscribe(resume)))
    }
    /// Publish only after the originating writer commits, preserving the commit's revisions.
    pub fn publish_event(
        &self,
        name: EventName,
        data: Payload,
        catalog_rev: Option<Generation>,
        state_rev: Option<Generation>,
    ) -> EngineResult<()> {
        self.events.publish(name, data, catalog_rev, state_rev)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn emit(bus: &Bus) {
        bus.publish(
            EventName::HealthWarning,
            serde_json::json!({"code":"scan_throttled","message":"test","at":timestamp()})
                .as_object()
                .unwrap()
                .clone(),
            None,
            None,
        )
        .unwrap();
    }
    fn event(s: &mut dyn EventStream) -> EventEnvelope {
        match s.recv_timeout(Duration::from_millis(100)) {
            RecvOutcome::Event(e) => {
                e.validate().unwrap();
                e
            }
            v => panic!("{v:?}"),
        }
    }
    #[test]
    fn resumes_inside_window_and_lagged_outside() {
        let bus = Arc::new(Bus::new(DatasetClass::Synthetic, Duration::from_secs(15)));
        emit(&bus);
        emit(&bus);
        let id = format!("{}.1", bus.epoch);
        let mut s = bus.subscribe(EventResume::from_last_event_id(&id));
        assert_eq!(event(&mut s).event, EventName::StreamHello);
        assert_eq!(event(&mut s).seq.get(), 2);
        for _ in 0..=EVENT_QUEUE_CAPACITY {
            emit(&bus);
        }
        let mut old = bus.subscribe(EventResume::from_last_event_id(&id));
        event(&mut old);
        assert_eq!(event(&mut old).data["reason"], "replay_gap");
        assert_eq!(event(&mut s).data["reason"], "subscriber_overflow");
    }
    #[test]
    fn heartbeat_after_configurable_silence_and_epoch_change() {
        let bus = Arc::new(Bus::new(DatasetClass::Synthetic, Duration::from_millis(10)));
        let mut s = bus.subscribe(Some(EventResume {
            epoch: "different_epoch".into(),
            last_seq: 0,
        }));
        event(&mut s);
        assert_eq!(event(&mut s).data["reason"], "epoch_changed");
        let now = Instant::now();
        assert_eq!(event(&mut s).event, EventName::StreamHello);
        assert!(now.elapsed() >= Duration::from_millis(9));
        s.close();
        assert!(matches!(
            s.recv_timeout(Duration::ZERO),
            RecvOutcome::Closed(_)
        ));
    }
}
#[cfg(test)]
mod replay_contract_tests {
    use super::*;
    #[test]
    fn heartbeat_is_unsequenced_and_not_replayed() {
        let bus = Arc::new(Bus::new(DatasetClass::Synthetic, Duration::from_millis(1)));
        let mut stream = bus.subscribe(None);
        stream.recv_timeout(Duration::ZERO);
        let RecvOutcome::Event(hello) = stream.recv_timeout(Duration::from_millis(100)) else {
            panic!("heartbeat missing")
        };
        assert_eq!(hello.event, EventName::StreamHello);
        assert_eq!(hello.seq.get(), 0);
        assert_eq!(bus.state.lock().unwrap().seq, 0);
        assert!(bus.state.lock().unwrap().replay.is_empty());
    }
}
#[cfg(test)]
mod expiry_tests {
    use super::*;
    #[test]
    fn reconnect_replay_expires_after_sixty_seconds() {
        let bus = Arc::new(Bus::new(DatasetClass::Synthetic, Duration::from_secs(15)));
        let mut live = bus.subscribe(None);
        live.recv_timeout(Duration::ZERO);
        bus.publish(
            EventName::HealthWarning,
            serde_json::json!({"code":"scan_throttled","message":"test","at":timestamp()})
                .as_object()
                .unwrap()
                .clone(),
            None,
            None,
        )
        .unwrap();
        bus.state
            .lock()
            .unwrap()
            .replay
            .front_mut()
            .unwrap()
            .published = Instant::now() - Duration::from_secs(61);
        let mut reconnect = bus.subscribe(Some(EventResume {
            epoch: bus.epoch.clone(),
            last_seq: 0,
        }));
        reconnect.recv_timeout(Duration::ZERO);
        let RecvOutcome::Event(gap) = reconnect.recv_timeout(Duration::ZERO) else {
            panic!("missing replay gap")
        };
        assert_eq!(gap.data["reason"], "replay_gap");
        assert!(
            matches!(live.recv_timeout(Duration::ZERO),RecvOutcome::Event(e) if e.event==EventName::HealthWarning)
        );
    }
}
