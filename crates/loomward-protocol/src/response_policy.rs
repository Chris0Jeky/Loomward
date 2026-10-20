//! Response admission across a forget decision. Unlike event suspension, a confirmed rejected
//! write may resume the same response generation. Durable, unknown and unwound outcomes retire it.

use std::collections::HashMap;
use std::future::poll_fn;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};

#[derive(Debug)]
struct State {
    current: Arc<()>,
    suspended: usize,
    waiters: HashMap<usize, Waker>,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn wake(&self, waiters: HashMap<usize, Waker>) {
        self.changed.notify_all();
        // Waking an adapter may immediately poll or otherwise re-enter the policy. No policy
        // mutex is held while calling, dropping or replacing a registered waker.
        for waker in waiters.into_values() {
            waker.wake();
        }
    }
}

/// A response-only policy shared by the service and its adapters. A pause retains the current
/// generation until the forget outcome is classified. Retired generations are never reused;
/// only pending responses retain them, with no root identifiers or generation history.
#[derive(Clone, Debug)]
pub struct ResponseDeliveryPolicy {
    shared: Arc<Shared>,
}

impl Default for ResponseDeliveryPolicy {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    current: Arc::new(()),
                    suspended: 0,
                    waiters: HashMap::new(),
                }),
                changed: Condvar::new(),
            }),
        }
    }
}

impl ResponseDeliveryPolicy {
    /// Capture before dispatch: stamping a completed reply can miss a concurrent forget.
    pub fn stamp(&self) -> ResponseDeliveryStamp {
        let generation = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current
            .clone();
        ResponseDeliveryStamp {
            shared: self.shared.clone(),
            generation,
            waiter: None,
        }
    }

    /// Retire every earlier stamp. A concurrent pause still prevents admission of new stamps.
    pub fn invalidate(&self) {
        let waiters = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.current = Arc::new(());
            std::mem::take(&mut state.waiters)
        };
        self.shared.wake(waiters);
    }

    /// Pause without retiring the current generation. The guard owns no lock while the writer
    /// runs. Only an explicitly confirmed rejection resumes it; Drop fails closed otherwise.
    pub fn suspend(&self) -> ResponseDeliverySuspension {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.suspended = state
            .suspended
            .checked_add(1)
            .expect("response delivery suspension count exhausted");
        ResponseDeliverySuspension {
            policy: self.clone(),
            rejected: false,
        }
    }
}

/// One pending reply. The admission check is the final boundary before handing the reply on;
/// the policy lock is released before that handoff, and no await follows a successful check.
#[derive(Debug)]
#[must_use]
pub struct ResponseDeliveryStamp {
    shared: Arc<Shared>,
    generation: Arc<()>,
    // A live allocation supplies a unique waiter key, without a counter that can wrap. It is
    // allocated only if this response actually waits and is removed when its future is canceled.
    waiter: Option<Arc<()>>,
}

impl ResponseDeliveryStamp {
    /// Synchronous adapters only: release service locks before calling. Async adapters must use
    /// admit_async so a pending forget never blocks their runtime thread.
    pub fn admit(self) -> bool {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            let current = Arc::ptr_eq(&self.generation, &state.current);
            if !current || state.suspended == 0 {
                return current;
            }
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    pub async fn admit_async(mut self) -> bool {
        poll_fn(|cx| self.poll_admit(cx)).await
    }

    fn poll_admit(&mut self, cx: &mut Context<'_>) -> Poll<bool> {
        // Waker operations are adapter callbacks too: clone before acquiring the policy lock.
        let waker = cx.waker().clone();
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = Arc::ptr_eq(&self.generation, &state.current);
        if !current || state.suspended == 0 {
            let previous = self
                .waiter
                .take()
                .and_then(|key| state.waiters.remove(&(Arc::as_ptr(&key) as usize)));
            drop(state);
            drop(previous);
            return Poll::Ready(current);
        }
        let key = self.waiter.get_or_insert_with(|| Arc::new(()));
        let previous = state.waiters.insert(Arc::as_ptr(key) as usize, waker);
        drop(state);
        drop(previous);
        Poll::Pending
    }
}

impl Drop for ResponseDeliveryStamp {
    fn drop(&mut self) {
        if let Some(key) = self.waiter.take() {
            let previous = {
                self.shared
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .waiters
                    .remove(&(Arc::as_ptr(&key) as usize))
            };
            drop(previous);
        }
    }
}

#[derive(Debug)]
#[must_use]
pub struct ResponseDeliverySuspension {
    policy: ResponseDeliveryPolicy,
    rejected: bool,
}

impl ResponseDeliverySuspension {
    /// Call only after proving the durable forget marker is absent. Other concurrent pauses
    /// must still finish, and a generation retired by another outcome can never be revived.
    pub fn resume(mut self) {
        self.rejected = true;
    }
}

impl Drop for ResponseDeliverySuspension {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self
                .policy
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !self.rejected {
                state.current = Arc::new(());
            }
            state.suspended = state
                .suspended
                .checked_sub(1)
                .expect("response delivery suspension is balanced");
            std::mem::take(&mut state.waiters)
        };
        self.policy.shared.wake(waiters);
    }
}

#[cfg(test)]
#[path = "response_policy_tests.rs"]
mod tests;
