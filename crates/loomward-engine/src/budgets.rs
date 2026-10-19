//! Bounded own-pool budgets and the shared staging byte semaphore.
use crate::{Engine, EngineResult};
use loomward_protocol::{BudgetSetRequest, BudgetSetRequestPool, OwnBudgets};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::Duration;

/// Default bytes shared by producers, arena reservations and queued staging chunks.
pub const SCAN_BYTES: usize = 64 * 1024 * 1024;
#[derive(Debug)]
pub(crate) struct Budgets {
    pub wire: Mutex<OwnBudgets>,
    pub bytes: Arc<ByteBudget>,
    scan_workers: Arc<AtomicUsize>,
}
impl Default for Budgets {
    fn default() -> Self {
        let wire = serde_json::from_value(serde_json::json!({"pools": [
            {"pool":"scan_enumerate","max_workers":8,"default_workers":4,"current_workers":0,"queue_capacity":1024,"scope":"loomward_own_threads"},
            {"pool":"learning","max_workers":2,"default_workers":2,"current_workers":0,"queue_capacity":32,"scope":"loomward_own_threads"},
            {"pool":"telemetry","max_workers":1,"default_workers":1,"current_workers":0,"queue_capacity":32,"scope":"loomward_own_threads"}],
            "teacher":{"max_in_flight":1,"timeout_s":180,"memory_limit_bytes":null,"enforcement":"not_available"},
            "enforcement_note":"Caps Loomward own pools; no OS control of other processes."})).expect("constant budgets DTO");
        Self {
            wire: Mutex::new(wire),
            bytes: Arc::new(ByteBudget::new(SCAN_BYTES)),
            scan_workers: Arc::new(AtomicUsize::new(0)),
        }
    }
}
pub(crate) struct WorkerPermit {
    active: Arc<AtomicUsize>,
    pub count: usize,
}
impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(self.count, Ordering::AcqRel);
    }
}
impl Budgets {
    pub fn reserve_scan_workers(
        &self,
        requested: Option<usize>,
    ) -> crate::EngineResult<WorkerPermit> {
        let wire = self.wire.lock().unwrap();
        let cap = wire
            .pools
            .iter()
            .find(|p| p.pool == loomward_protocol::PoolName::ScanEnumerate)
            .unwrap()
            .max_workers
            .get() as usize;
        let current = self.scan_workers.load(Ordering::Acquire);
        // Default scans (including freshness) leave headroom for another root's work.
        let count = requested
            .unwrap_or_else(|| cap.div_ceil(2))
            .min(cap.saturating_sub(current));
        if count == 0 {
            return Err(crate::EngineError::ResourceBudget {
                message: "all scan workers are in use; wait for a running scan to finish or cancel it, then retry".into(),
            });
        }
        self.scan_workers.fetch_add(count, Ordering::AcqRel);
        Ok(WorkerPermit {
            active: self.scan_workers.clone(),
            count,
        })
    }
}

/// One byte semaphore shared across all active scans. Permits live through sink staging.
#[derive(Debug)]
pub struct ByteBudget {
    capacity: usize,
    used: Mutex<usize>,
    changed: Condvar,
}
impl ByteBudget {
    /// A finite staging budget.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            used: Mutex::new(0),
            changed: Condvar::new(),
        }
    }
    /// Total byte capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    /// Current reserved bytes.
    pub fn used(&self) -> usize {
        *self.used.lock().unwrap()
    }
    /// Cancellable acquisition; writer failure also aborts the wait.
    pub fn acquire(
        self: &Arc<Self>,
        bytes: usize,
        cancel: &AtomicBool,
        failed: &AtomicBool,
    ) -> Option<BytePermit> {
        if bytes > self.capacity {
            return None;
        }
        let mut used = self.used.lock().unwrap();
        loop {
            if cancel.load(Ordering::Acquire) || failed.load(Ordering::Acquire) {
                return None;
            }
            if bytes <= self.capacity - *used {
                *used += bytes;
                return Some(BytePermit {
                    budget: self.clone(),
                    bytes,
                });
            }
            used = self
                .changed
                .wait_timeout(used, Duration::from_millis(5))
                .unwrap()
                .0;
        }
    }
}
/// Reservation released only after the writer consumes or drops a message.
#[derive(Debug)]
pub struct BytePermit {
    budget: Arc<ByteBudget>,
    bytes: usize,
}
impl Drop for BytePermit {
    fn drop(&mut self) {
        *self.budget.used.lock().unwrap() -= self.bytes;
        self.budget.changed.notify_all();
    }
}
impl Engine {
    /// Current own-pool caps.
    pub fn budgets_get(&self) -> EngineResult<OwnBudgets> {
        let mut wire = self.budgets.wire.lock().unwrap().clone();
        wire.pools = loomward_protocol::BoundedVec::new(
            wire.pools
                .iter()
                .cloned()
                .map(|mut p| {
                    if p.pool == loomward_protocol::PoolName::ScanEnumerate {
                        p.default_workers = loomward_protocol::Int::new(
                            (p.max_workers.get() as usize).div_ceil(2) as i64,
                        )
                        .unwrap();
                        p.current_workers = loomward_protocol::Int::new(
                            self.budgets.scan_workers.load(Ordering::Acquire) as i64,
                        )
                        .unwrap();
                    }
                    p
                })
                .collect(),
        )
        .unwrap();
        Ok(wire)
    }
    /// Sets a worker cap for subsequent jobs; never alters OS process policy.
    pub fn budgets_set(&self, request: &BudgetSetRequest) -> EngineResult<OwnBudgets> {
        let mut wire = self.budgets.wire.lock().unwrap();
        let name = match request.pool {
            BudgetSetRequestPool::ScanEnumerate => loomward_protocol::PoolName::ScanEnumerate,
            BudgetSetRequestPool::Learning => loomward_protocol::PoolName::Learning,
            BudgetSetRequestPool::Telemetry => loomward_protocol::PoolName::Telemetry,
        };
        wire.pools = loomward_protocol::BoundedVec::new(
            wire.pools
                .iter()
                .cloned()
                .map(|mut p| {
                    if p.pool == name {
                        p.max_workers =
                            loomward_protocol::Int::new(request.max_workers.get()).unwrap();
                    }
                    p
                })
                .collect(),
        )
        .unwrap();
        drop(wire);
        self.budgets_get()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn issue776_default_scan_leaves_headroom_and_explicit_threads_are_a_ceiling() {
        let budget = Budgets::default();
        let first = budget.reserve_scan_workers(None).unwrap();
        assert_eq!(first.count, 4);
        let second = budget.reserve_scan_workers(Some(1)).unwrap();
        assert_eq!(second.count, 1);
        let refresh = budget.reserve_scan_workers(None).unwrap();
        assert_eq!(refresh.count, 3);
        assert!(budget.reserve_scan_workers(Some(1)).is_err());
        drop(refresh);
        let clamped = budget.reserve_scan_workers(Some(32)).unwrap();
        assert_eq!(clamped.count, 3);
        drop((first, second, clamped));
        assert_eq!(budget.scan_workers.load(Ordering::Acquire), 0);
    }

    #[test]
    fn issue776_defaults_follow_small_caps_and_cap_reductions() {
        let engine = Engine::open(crate::EngineConfig::new(
            "unused".into(),
            loomward_protocol::DatasetClass::Synthetic,
        ))
        .unwrap();
        for cap in [1, 2, 3, 8] {
            let wire = engine
                .budgets_set(
                    &serde_json::from_value(serde_json::json!({
                        "pool":"scan_enumerate", "max_workers":cap
                    }))
                    .unwrap(),
                )
                .unwrap();
            let first = engine.budgets.reserve_scan_workers(None).unwrap();
            assert_eq!(first.count, (cap as usize).div_ceil(2));
            assert_eq!(wire.pools[0].default_workers.get() as usize, first.count);
            engine
                .budgets_set(
                    &serde_json::from_value(serde_json::json!({
                        "pool":"scan_enumerate", "max_workers":1
                    }))
                    .unwrap(),
                )
                .unwrap();
            assert!(engine.budgets.reserve_scan_workers(None).is_err());
            drop(first);
            assert_eq!(engine.budgets.reserve_scan_workers(None).unwrap().count, 1);
        }
    }

    #[test]
    fn shared_bytes_wait_is_cancellable_and_permits_release() {
        let budget = Arc::new(ByteBudget::new(32));
        let cancel = Arc::new(AtomicBool::new(false));
        let fail = AtomicBool::new(false);
        let held = budget.acquire(32, &cancel, &fail).unwrap();
        let b = budget.clone();
        let c = cancel.clone();
        let waiting = std::thread::spawn(move || b.acquire(1, &c, &AtomicBool::new(false)));
        cancel.store(true, Ordering::Release);
        assert!(waiting.join().unwrap().is_none());
        drop(held);
        assert_eq!(budget.used(), 0);
    }
}
