//! In-process event admission, separate from the wire envelope. Adapters can hold an owned event
//! after the service redacts its replay buffer; a stamp prevents that old copy being handed on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct State {
    current: Arc<AtomicBool>,
    suspended: usize,
}

impl State {
    fn replace_stamp(&mut self) {
        self.current.store(false, Ordering::Release);
        self.current = Arc::new(AtomicBool::new(self.suspended == 0));
    }
}

/// Shared by an event source and its adapters. Each adapter keeps one policy and stamps a receive
/// before it starts. Invalidating the policy retires every earlier stamp, without retaining root
/// identifiers or a history of generations. Memory is bounded by the adapter's pending work.
#[derive(Clone, Debug)]
pub struct EventDeliveryPolicy {
    state: Arc<Mutex<State>>,
}

impl Default for EventDeliveryPolicy {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                current: Arc::new(AtomicBool::new(true)),
                suspended: 0,
            })),
        }
    }
}

impl EventDeliveryPolicy {
    /// Capture BEFORE recv_timeout. Capturing only after receiving could stamp an old owned
    /// envelope with the new policy after a concurrent redaction.
    pub fn stamp(&self) -> EventDeliveryStamp {
        EventDeliveryStamp(
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .current
                .clone(),
        )
    }

    /// Retire pending copies when the source redacts its retained events. During a suspension,
    /// this cannot accidentally re-enable admission.
    pub fn invalidate(&self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .replace_stamp();
    }

    /// Suspend admission while a privacy mutation can commit. The guard owns no mutex, so it can
    /// span writer work without holding a lock across an await or a native emit. Dropping the last
    /// guard enables only newly captured stamps; every older stamp remains invalid.
    pub fn suspend(&self) -> EventDeliverySuspension {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.suspended = state
            .suspended
            .checked_add(1)
            .expect("event delivery suspension count exhausted");
        state.replace_stamp();
        EventDeliverySuspension {
            policy: self.clone(),
        }
    }
}

/// An adapter-owned receive's admission stamp. A successful check is the admission boundary:
/// serialize first, then check immediately before yielding bytes to the HTTP body or calling a
/// native sink, with no await between. Neither this policy nor a later forget can retract bytes
/// already admitted to the transport, its buffers, or a client. No service lock spans that handoff.
#[derive(Debug)]
#[must_use]
pub struct EventDeliveryStamp(Arc<AtomicBool>);

impl EventDeliveryStamp {
    pub fn admit(self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
#[must_use]
pub struct EventDeliverySuspension {
    policy: EventDeliveryPolicy,
}

impl Drop for EventDeliverySuspension {
    fn drop(&mut self) {
        let mut state = self
            .policy
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.suspended = state
            .suspended
            .checked_sub(1)
            .expect("event delivery suspension is balanced");
        state.replace_stamp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_never_revalidates_an_older_owned_event() {
        let policy = EventDeliveryPolicy::default();
        let before_receive = policy.stamp();
        policy.invalidate();
        assert!(!before_receive.admit());
        assert!(policy.stamp().admit());
        let pending = policy.stamp();
        for _ in 0..100 {
            policy.invalidate();
        }
        assert!(
            !pending.admit(),
            "a retired generation must never become current again"
        );
    }

    #[test]
    fn nested_suspensions_and_redaction_resume_only_new_receives() {
        let policy = EventDeliveryPolicy::default();
        let queued = policy.stamp();
        let first = policy.suspend();
        let during = policy.stamp();
        let second = policy.suspend();
        policy.invalidate();
        assert!(!queued.admit());
        assert!(!policy.stamp().admit());
        drop(first);
        assert!(
            !policy.stamp().admit(),
            "one privacy mutation is still in progress"
        );
        drop(second);
        assert!(!during.admit(), "a suspended receive cannot be revived");
        assert!(policy.stamp().admit());
    }
}
