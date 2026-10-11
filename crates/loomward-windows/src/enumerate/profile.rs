//! Opt-in, aggregate-only scan diagnostics. No paths, IDs or names leave this module.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};
use std::time::Instant;

pub(super) struct Phase {
    calls: AtomicU64,
    nanos: AtomicU64,
}
impl Phase {
    const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
        }
    }
    pub(super) fn start(&'static self) -> Timer {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        Timer(
            self,
            (*ENABLED.get_or_init(|| std::env::var_os("LOOMWARD_SCAN_PROFILE").is_some()))
                .then(Instant::now),
        )
    }
    fn value(&self) -> serde_json::Value {
        serde_json::json!({"calls": self.calls.load(Ordering::Relaxed), "seconds": self.nanos.load(Ordering::Relaxed) as f64 / 1e9})
    }
}
pub(super) struct Timer(&'static Phase, Option<Instant>);
impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            self.0.calls.fetch_add(1, Ordering::Relaxed);
            self.0.nanos.fetch_add(
                start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                Ordering::Relaxed,
            );
        }
    }
}
pub(super) static LIST: Phase = Phase::new();
pub(super) static CALLBACK: Phase = Phase::new();
pub(super) static OPEN: Phase = Phase::new();
pub(super) static IDENTITY: Phase = Phase::new();
pub(super) static ATTRIBUTES: Phase = Phase::new();
pub(super) static CONTAINMENT: Phase = Phase::new();
pub(super) static PATH_QUERY: Phase = Phase::new();
pub(super) static ENUM_QUERY: Phase = Phase::new();

/// Process-cumulative counters for a fresh-process scan benchmark; nested times overlap.
pub fn scan_profile() -> serde_json::Value {
    serde_json::json!({"listing_inclusive": LIST.value(), "entry_callback": CALLBACK.value(), "metadata_opens": OPEN.value(), "identity_observations": IDENTITY.value(), "attribute_queries": ATTRIBUTES.value(), "containment": CONTAINMENT.value(), "normalized_path_queries": PATH_QUERY.value(), "enumeration_queries": ENUM_QUERY.value()})
}
