//! The session idempotency cache (semantics section 4, errata #117 N4). One slot per `request_id`
//! of a mutating call. The first call claims the slot and runs; a byte-identical retry waits for
//! and replays the retained outcome, never executing twice; the same `request_id` with a different
//! request is an `idempotency_conflict`. Outcomes are kept for the TTL after they complete; a slot
//! still running never expires. At most [`MAX_SLOTS`] slots are live: a new `request_id` beyond
//! that is refused before any work runs, and no running or unexpired slot is ever evicted.

use loomward_protocol::ResponseEnvelope;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

pub const TTL: Duration = Duration::from_secs(600);
pub const MAX_SLOTS: usize = 1024;

/// SHA-256 of the canonical request JSON (object keys sorted), not the request itself.
fn digest(fingerprint: &Value) -> [u8; 32] {
    Sha256::digest(fingerprint.to_string()).into()
}

struct Slot {
    fingerprint: [u8; 32],
    outcome: Option<(ResponseEnvelope, Instant)>,
}

/// What a caller does with its `request_id`.
#[derive(Debug, PartialEq)]
pub enum Claim {
    /// First sight: run the mutation, then [`Idempotency::finish`].
    Run,
    /// Already claimed by an identical request: [`Idempotency::wait`] for its outcome.
    Wait,
    /// Claimed by a different request.
    Conflict,
    /// A new `request_id` while [`MAX_SLOTS`] slots are live: refused, nothing claimed.
    Full,
}

pub struct Idempotency {
    ttl: Duration,
    max: usize,
    slots: Mutex<HashMap<String, Slot>>,
    done: Condvar,
}

impl Idempotency {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            max: MAX_SLOTS,
            slots: Mutex::new(HashMap::new()),
            done: Condvar::new(),
        }
    }

    pub fn claim(&self, request_id: &str, fingerprint: &Value) -> Claim {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let ttl = self.ttl;
        // ponytail: O(MAX_SLOTS) sweep per claim; an expiry queue if the cap grows.
        slots.retain(|_, s| s.outcome.as_ref().is_none_or(|(_, at)| at.elapsed() < ttl));
        let fingerprint = digest(fingerprint);
        match slots.get(request_id) {
            Some(slot) if slot.fingerprint == fingerprint => Claim::Wait,
            Some(_) => Claim::Conflict,
            None if slots.len() >= self.max => Claim::Full,
            None => {
                slots.insert(
                    request_id.to_owned(),
                    Slot {
                        fingerprint,
                        outcome: None,
                    },
                );
                Claim::Run
            }
        }
    }

    /// Frees a slot whose claim was refused before any work ran.
    pub fn release(&self, request_id: &str) {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id);
        self.done.notify_all();
    }

    pub fn finish(&self, request_id: &str, outcome: ResponseEnvelope) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = slots.get_mut(request_id) {
            slot.outcome = Some((outcome, Instant::now()));
        }
        drop(slots);
        self.done.notify_all();
    }

    /// The retained outcome, waiting until `until` for a running one. `None`: outcome unknown.
    pub fn wait(&self, request_id: &str, until: Instant) -> Option<ResponseEnvelope> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match slots.get(request_id) {
                Some(Slot {
                    outcome: Some((r, _)),
                    ..
                }) => return Some(r.clone()),
                Some(_) => {}
                None => return None,
            }
            let now = Instant::now();
            if now >= until {
                return None;
            }
            slots = self
                .done
                .wait_timeout(slots, until - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomward_protocol::{ErrorBody, ErrorCode, RequestId};
    use serde_json::json;
    use std::sync::Arc;

    fn outcome(n: u64) -> ResponseEnvelope {
        let body = ErrorBody::new(ErrorCode::Busy, &n.to_string(), true);
        ResponseEnvelope::error(Some(RequestId::new("r").unwrap()), body, None)
    }

    #[test]
    fn identical_retries_replay_conflicts_refuse_and_outcomes_expire() {
        let cache = Idempotency::new(Duration::from_millis(50));
        let a = json!({"command": "volumes.declare_tier", "payload": {"tier": 2}});
        let b = json!({"command": "volumes.declare_tier", "payload": {"tier": 5}});
        assert_eq!(cache.claim("r1", &a), Claim::Run);
        assert_eq!(cache.claim("r1", &a), Claim::Wait);
        assert_eq!(cache.claim("r1", &b), Claim::Conflict);
        // Running: a waiter times out with the outcome unknown.
        assert_eq!(
            cache.wait("r1", Instant::now() + Duration::from_millis(5)),
            None
        );
        cache.finish("r1", outcome(1));
        assert_eq!(cache.wait("r1", Instant::now()), Some(outcome(1)));
        std::thread::sleep(Duration::from_millis(60));
        // Expired: the key is free again (recovery now reads state, section 3 step 2).
        assert_eq!(cache.claim("r1", &b), Claim::Run);
    }

    /// M1: live slots are capped; a full cache refuses new keys, never evicts a running or
    /// unexpired slot, and frees space only as outcomes expire.
    #[test]
    fn live_slots_are_capped_and_never_evicted_early() {
        let mut cache = Idempotency::new(Duration::from_millis(50));
        assert_eq!(cache.max, MAX_SLOTS);
        cache.max = 2;
        let fp = json!(1);
        assert_eq!(cache.claim("a", &fp), Claim::Run);
        assert_eq!(cache.claim("b", &fp), Claim::Run);
        assert_eq!(cache.claim("c", &fp), Claim::Full);
        // Known keys still replay or conflict while full.
        assert_eq!(cache.claim("a", &fp), Claim::Wait);
        assert_eq!(cache.claim("b", &json!(2)), Claim::Conflict);
        cache.finish("a", outcome(1));
        assert_eq!(cache.claim("c", &fp), Claim::Full, "unexpired outcome kept");
        assert_eq!(cache.wait("a", Instant::now()), Some(outcome(1)));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(cache.claim("c", &fp), Claim::Run, "expired outcome freed");
        assert_eq!(
            cache.claim("d", &fp),
            Claim::Full,
            "running b never expires"
        );
        cache.release("c");
        assert_eq!(cache.claim("d", &fp), Claim::Run);
    }

    #[test]
    fn a_waiter_wakes_when_the_running_call_finishes() {
        let cache = Arc::new(Idempotency::new(TTL));
        let fp = json!(1);
        assert_eq!(cache.claim("r", &fp), Claim::Run);
        let c = cache.clone();
        let t = std::thread::spawn(move || c.wait("r", Instant::now() + Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(20));
        cache.finish("r", outcome(7));
        assert_eq!(t.join().unwrap(), Some(outcome(7)));
    }
}
