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
    privacy: ReplyPrivacy,
    withheld: bool,
}

/// Cached results carrying root or volume labels. No request paths are retained: the volume
/// key here is the bounded session ID already present in a tier request. Alert reports may
/// contain labels from every active root/volume, so their whole reply is conservatively scoped.
enum ReplyPrivacy {
    Other,
    RootGrant,
    Volume(String),
    Alerts,
}

impl ReplyPrivacy {
    fn from_request(fingerprint: &Value) -> Self {
        match fingerprint.get("command").and_then(Value::as_str) {
            Some("roots.request_grant") => Self::RootGrant,
            Some("alerts.configure") => Self::Alerts,
            Some("volumes.declare_tier" | "volumes.declare_reserve") => fingerprint
                .get("payload")
                .and_then(|p| p.get("volume_id"))
                .and_then(Value::as_str)
                .map_or(Self::Other, |id| Self::Volume(id.to_owned())),
            _ => Self::Other,
        }
    }
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
        self.claim_in(request_id, fingerprint, true)
    }

    /// For revocation only: never [`Claim::Full`], so a flood of other mutations cannot refuse
    /// the owner's revocation.
    // ponytail: revocation slots are uncapped; a revocation flood grows the cache for one TTL.
    pub fn claim_uncapped(&self, request_id: &str, fingerprint: &Value) -> Claim {
        self.claim_in(request_id, fingerprint, false)
    }

    fn claim_in(&self, request_id: &str, fingerprint: &Value, capped: bool) -> Claim {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let ttl = self.ttl;
        // ponytail: O(MAX_SLOTS) sweep per claim; an expiry queue if the cap grows.
        slots.retain(|_, s| s.outcome.as_ref().is_none_or(|(_, at)| at.elapsed() < ttl));
        let privacy = ReplyPrivacy::from_request(fingerprint);
        let fingerprint = digest(fingerprint);
        match slots.get(request_id) {
            Some(slot) if slot.fingerprint == fingerprint => Claim::Wait,
            Some(_) => Claim::Conflict,
            None if capped && slots.len() >= self.max => Claim::Full,
            None => {
                slots.insert(
                    request_id.to_owned(),
                    Slot {
                        fingerprint,
                        outcome: None,
                        privacy,
                        withheld: false,
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
            let outcome = if slot.withheld {
                outcome.without_forgotten_result()
            } else {
                outcome
            };
            slot.outcome = Some((outcome, Instant::now()));
        }
        drop(slots);
        self.done.notify_all();
    }

    /// Privacy withdraws a saved reply, never its action key. Complete slots keep their original
    /// completion timestamp. Running slots stay running and cannot expire/reexecute a picker;
    /// the flag prevents a later Settle::drop from restoring an earlier private snapshot.
    ///
    /// A pending picker has no root ID yet, and an alert report can name multiple roots/volumes.
    /// These slots crossing forget are conservatively withheld on completion; the owner recovers
    /// by reading state, without reopening a dialog or repeating an earlier settings mutation.
    pub fn forget_root(&self, root_id: &str, volume_id: Option<&str>) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for slot in slots.values_mut() {
            let affected = match &slot.privacy {
                ReplyPrivacy::Other => false,
                ReplyPrivacy::Alerts => true,
                ReplyPrivacy::Volume(id) => volume_id == Some(id.as_str()),
                ReplyPrivacy::RootGrant => match &slot.outcome {
                    None => true,
                    Some((ResponseEnvelope::Ok(reply), _)) => {
                        reply
                            .result
                            .get("root")
                            .and_then(|root| root.get("root_id"))
                            .and_then(Value::as_str)
                            == Some(root_id)
                    }
                    Some((ResponseEnvelope::Err(_), _)) => false,
                },
            };
            if affected {
                slot.withheld = true;
                slot.outcome = slot
                    .outcome
                    .take()
                    .map(|(reply, completed)| (reply.without_forgotten_result(), completed));
            }
        }
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

    fn root_outcome(key: &str, root: &str) -> ResponseEnvelope {
        let meta = serde_json::from_value(json!({
            "served_at":"2026-10-19T00:00:00Z", "elapsed_ms":0,
            "dataset_class":"synthetic", "budget_hit":false,
            "catalog_rev":"1", "state_rev":"1"
        }))
        .unwrap();
        ResponseEnvelope::ok_object(
            RequestId::new(key).unwrap(),
            json!({"outcome":"granted", "root":{"root_id":root, "display_path":"SyntheticPrivateRoot"}, "refusal":null})
                .as_object().unwrap().clone(),
            meta,
        )
    }

    #[test]
    fn retained_privacy_completed_slots_preserve_expiry_conflicts_and_other_roots() {
        let cache = Idempotency::new(TTL);
        let fingerprint =
            json!({"command":"roots.request_grant", "payload":{"purpose":"metadata_scan"}});
        for (key, root) in [("forgotten", "rt_forgotten"), ("kept", "rt_kept")] {
            assert_eq!(cache.claim(key, &fingerprint), Claim::Run);
            cache.finish(key, root_outcome(key, root));
        }
        let original_at = Instant::now() - Duration::from_secs(30);
        cache
            .slots
            .lock()
            .unwrap()
            .get_mut("forgotten")
            .unwrap()
            .outcome
            .as_mut()
            .unwrap()
            .1 = original_at;
        cache.forget_root("rt_forgotten", None);
        let erased = cache.wait("forgotten", Instant::now()).unwrap();
        let ResponseEnvelope::Err(error) = &erased else {
            panic!("cached path remained")
        };
        assert_eq!(error.error.code, ErrorCode::PermissionDenied);
        assert!(!error.error.retryable);
        assert_eq!(cache.claim("forgotten", &fingerprint), Claim::Wait);
        assert_eq!(
            cache.claim("forgotten", &json!({"different":"intent"})),
            Claim::Conflict
        );
        assert_eq!(
            cache.wait("kept", Instant::now()),
            Some(root_outcome("kept", "rt_kept"))
        );
        cache.forget_root("rt_forgotten", None);
        assert_eq!(cache.wait("forgotten", Instant::now()), Some(erased));
        assert_eq!(
            cache.slots.lock().unwrap()["forgotten"]
                .outcome
                .as_ref()
                .unwrap()
                .1,
            original_at
        );
    }

    #[test]
    fn retained_privacy_running_picker_stays_claimed_until_its_actual_finish() {
        // A zero TTL makes any accidentally completed slot expire on the very next claim.
        let cache = Idempotency::new(Duration::ZERO);
        let fingerprint =
            json!({"command":"roots.request_grant", "payload":{"purpose":"metadata_scan"}});
        assert_eq!(cache.claim("picker", &fingerprint), Claim::Run);
        cache.forget_root("rt_forgotten", None);
        assert_eq!(
            cache.wait("picker", Instant::now()),
            None,
            "forget does not invent completion"
        );
        assert_eq!(
            cache.claim("picker", &fingerprint),
            Claim::Wait,
            "a pending dialog cannot expire and run twice"
        );
        assert_eq!(
            cache.claim("picker", &json!({"different":"intent"})),
            Claim::Conflict
        );
        let finishing = Instant::now();
        cache.finish("picker", root_outcome("picker", "rt_forgotten"));
        let ResponseEnvelope::Err(error) = cache.wait("picker", Instant::now()).unwrap() else {
            panic!("late completion restored its path")
        };
        assert_eq!(error.error.detail.unwrap()["reason"], "forgotten_result");
        assert!(
            cache.slots.lock().unwrap()["picker"]
                .outcome
                .as_ref()
                .unwrap()
                .1
                >= finishing
        );
    }

    #[test]
    fn retained_privacy_volume_invalidation_is_scoped_to_its_original_request() {
        let cache = Idempotency::new(TTL);
        for (key, volume) in [("affected", "vo_1"), ("unrelated", "vo_2")] {
            let request =
                json!({"command":"volumes.declare_tier", "payload":{"volume_id":volume,"tier":2}});
            assert_eq!(cache.claim(key, &request), Claim::Run);
            cache.finish(key, root_outcome(key, "rt_fixture"));
        }
        cache.forget_root("rt_forgotten", Some("vo_1"));
        assert!(matches!(
            cache.wait("affected", Instant::now()),
            Some(ResponseEnvelope::Err(_))
        ));
        assert_eq!(
            cache.wait("unrelated", Instant::now()),
            Some(root_outcome("unrelated", "rt_fixture"))
        );
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
