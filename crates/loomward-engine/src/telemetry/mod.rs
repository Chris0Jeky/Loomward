//! Read-only native observations on one lease-driven thread (LW-050, LW-107).
//! No lease means no sampler. Snapshots read the latest retained observation, including its age.
//! The service/event-bus lane drains bounded subscription samples via `telemetry_events`.
mod mapping;

use crate::{Engine, EngineError, EngineResult};
use loomward_protocol::*;
use loomward_telemetry::{Process, Sampler, Snapshot};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LEASE_DURATION: Duration = Duration::from_secs(60);
const MAX_LEASES: usize = 32;
const EVENT_CAPACITY: usize = 1024;

#[derive(Debug)]
struct Lease {
    channels: UniqueVec<TelemetryChannel, 1, 5>,
    interval: TelemetryInterval,
    expires: Instant,
    next_event: Instant,
}

#[derive(Debug)]
struct Latest {
    raw: Snapshot,
    own: Option<Process>,
    sequence: u64,
}

#[derive(Debug, Default)]
struct State {
    leases: BTreeMap<SubscriptionId, Lease>,
    latest: Option<Latest>,
    events: VecDeque<TelemetrySampleEvent>,
    next_id: u64,
    sequence: u64,
    running: bool,
    busy: bool,
    shutdown: bool,
}

impl State {
    fn expire(&mut self, now: Instant) {
        self.leases.retain(|_, lease| lease.expires > now);
        self.events
            .retain(|event| self.leases.contains_key(&event.subscription_id));
    }

    fn latest(&self) -> EngineResult<&Latest> {
        self.latest
            .as_ref()
            .ok_or_else(|| EngineError::PartialCoverage {
                message: "no telemetry sample yet; a live subscription is required".into(),
            })
    }
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// Private engine state; the OS sampler is created and destroyed on its dedicated thread.
#[derive(Debug, Default)]
pub(crate) struct Telemetry {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> EngineResult<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| EngineError::Internal {
        message: "telemetry state poisoned".into(),
    })
}

fn not_found() -> EngineError {
    EngineError::NotFound {
        message: "telemetry subscription or process instance not found".into(),
    }
}

impl Telemetry {
    fn lease(&self, request: &TelemetrySubscribeRequest) -> EngineResult<TelemetrySubscription> {
        // Serialises startup, last-release join and renewal; sampler never takes the worker lock.
        let mut worker = lock(&self.worker)?;
        let mut state = lock(&self.shared.state)?;
        let now = Instant::now();
        state.expire(now);
        let id =
            match &request.subscription_id {
                Some(id) if state.leases.contains_key(id) => id.clone(),
                Some(_) => return Err(not_found()),
                None => {
                    if state.leases.len() >= MAX_LEASES {
                        return Err(EngineError::ResourceBudget {
                            message: "telemetry lease limit (32)".into(),
                        });
                    }
                    state.next_id = state.next_id.checked_add(1).ok_or_else(|| {
                        EngineError::ResourceBudget {
                            message: "telemetry subscription ids exhausted".into(),
                        }
                    })?;
                    SubscriptionId::new(format!("sb_telemetry_{}", state.next_id))
                        .map_err(mapping::invalid)?
                }
            };
        let unix =
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| EngineError::Internal {
                    message: "system clock before Unix epoch".into(),
                })?;
        let expires_at = mapping::timestamp((unix + LEASE_DURATION).as_nanos() as i128 / 100)?;
        state.leases.insert(
            id.clone(),
            Lease {
                channels: request.channels.clone(),
                interval: request.interval_ms,
                expires: now + LEASE_DURATION,
                next_event: now,
            },
        );
        if !state.running {
            if let Some(old) = worker.take() {
                drop(state);
                old.join().map_err(|_| EngineError::Internal {
                    message: "telemetry sampler panicked".into(),
                })?;
                state = lock(&self.shared.state)?;
            }
            state.running = true;
            let shared = Arc::clone(&self.shared);
            match thread::Builder::new()
                .name("loomward-telemetry".into())
                .spawn(move || run(shared))
            {
                Ok(handle) => *worker = Some(handle),
                Err(_) => {
                    state.running = false;
                    state.leases.remove(&id);
                    return Err(EngineError::Internal {
                        message: "telemetry sampler thread could not start".into(),
                    });
                }
            }
        }
        self.shared.wake.notify_one();
        Ok(TelemetrySubscription {
            subscription_id: id,
            channels: request.channels.clone(),
            interval_ms: request.interval_ms,
            expires_at,
        })
    }

    fn release(&self, request: &SubscriptionRefRequest) -> EngineResult<TelemetryUnsubscribed> {
        let mut worker = lock(&self.worker)?;
        let mut state = lock(&self.shared.state)?;
        state.expire(Instant::now());
        if state.leases.remove(&request.subscription_id).is_none() {
            return Err(not_found());
        }
        state
            .events
            .retain(|e| e.subscription_id != request.subscription_id);
        let last = state.leases.is_empty();
        self.shared.wake.notify_one();
        drop(state);
        if last {
            if let Some(handle) = worker.take() {
                handle.join().map_err(|_| EngineError::Internal {
                    message: "telemetry sampler panicked".into(),
                })?;
            }
        }
        Ok(TelemetryUnsubscribed {
            subscription_id: request.subscription_id.clone(),
            stopped: true,
        })
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        // Poisoned state must still let the owning engine shut down its thread.
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.shutdown = true;
        self.shared.wake.notify_one();
        drop(state);
        if let Some(handle) = self
            .worker
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = handle.join();
        }
    }
}

fn run(shared: Arc<Shared>) {
    let mut sampler = Sampler::default();
    let mut next_sample = Instant::now();
    let mut previous_sample: Option<(Instant, Instant)> = None;
    loop {
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        state.expire(now);
        if state.shutdown || state.leases.is_empty() {
            state.running = false;
            state.busy = false;
            return;
        }
        let interval = Duration::from_millis(u64::from(
            state
                .leases
                .values()
                .map(|l| l.interval.millis())
                .min()
                .unwrap(),
        ));
        if let Some((started, finished)) = previous_sample {
            next_sample = if started + interval > finished {
                started + interval
            } else {
                finished + interval
            };
        }
        let next_expiry = state.leases.values().map(|l| l.expires).min().unwrap();
        let deadline = next_sample.min(next_expiry);
        if deadline > now {
            let _ = shared
                .wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            continue;
        }
        state.busy = true;
        drop(state);
        let started = Instant::now();
        let raw = sampler.sample(loomward_telemetry::MAX_PROCESS_ROWS);
        let own = sampler.own_process().cloned();
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.busy = false;
        let finished = Instant::now();
        state.expire(finished);
        if !state.shutdown && !state.leases.is_empty() {
            state.sequence = state.sequence.saturating_add(1);
            let sequence = state.sequence;
            let mut events = vec![];
            for (id, lease) in &mut state.leases {
                if lease.next_event <= started {
                    if let Ok(sample) =
                        mapping::sample(&raw, own.as_ref(), sequence, &lease.channels, true)
                    {
                        events.push(TelemetrySampleEvent {
                            subscription_id: id.clone(),
                            sample,
                        });
                    }
                    lease.next_event =
                        started + Duration::from_millis(u64::from(lease.interval.millis()));
                }
            }
            for event in events {
                if state.events.len() == EVENT_CAPACITY {
                    state.events.pop_front();
                }
                state.events.push_back(event);
            }
            state.latest = Some(Latest { raw, own, sequence });
        }
        previous_sample = Some((started, finished));
        next_sample = (started + interval).max(finished);
        // A sampler slower than its lease interval must not turn into a tight loop.
        if next_sample == finished {
            next_sample += interval;
        }
        shared.wake.notify_all();
    }
}

impl Engine {
    /// Creates/renews a 60 s lease. All leases share a sampler at the fastest live interval.
    pub fn telemetry_lease(
        &self,
        request: &TelemetrySubscribeRequest,
    ) -> EngineResult<TelemetrySubscription> {
        if self.config().dataset_class == DatasetClass::Synthetic {
            return Err(EngineError::PermissionDenied {
                message: "synthetic sessions cannot observe personal host telemetry".into(),
            });
        }
        self.telemetry.lease(request)
    }

    /// Stops this subscription; releasing the last lease joins even in-flight sampling.
    pub fn telemetry_release(
        &self,
        request: &SubscriptionRefRequest,
    ) -> EngineResult<TelemetryUnsubscribed> {
        self.telemetry.release(request)
    }

    /// Latest retained sample, projected to requested channels. Never starts an on-demand scan.
    pub fn telemetry_snapshot(
        &self,
        request: &TelemetrySnapshotRequest,
    ) -> EngineResult<TelemetrySample> {
        let state = lock(&self.telemetry.shared.state)?;
        let latest = state.latest()?;
        mapping::sample(
            &latest.raw,
            latest.own.as_ref(),
            latest.sequence,
            &request.channels,
            state.busy,
        )
    }

    /// Drains up to 1,024 pending lease samples for the service's event bus.
    /// Oldest samples drop at capacity; expired/released leases emit no queued events.
    pub fn telemetry_events(&self) -> EngineResult<Vec<TelemetrySampleEvent>> {
        let mut state = lock(&self.telemetry.shared.state)?;
        state.expire(Instant::now());
        Ok(state.events.drain(..).collect())
    }

    /// Own-process cumulative CPU seconds, commit and residency from the latest observation.
    pub fn telemetry_health(&self) -> EngineResult<HealthEngine> {
        let state = lock(&self.telemetry.shared.state)?;
        Ok(mapping::health(state.latest()?.own.as_ref()))
    }

    /// Bounded process display; unknown sort values come last, PID breaks ties.
    pub fn processes_list(&self, request: &ProcessListRequest) -> EngineResult<ProcessList> {
        let state = lock(&self.telemetry.shared.state)?;
        let latest = state.latest()?;
        if latest.raw.status == "unsupported"
            || !latest.raw.process_totals.enumeration_unknowns.is_empty()
        {
            return Err(EngineError::PartialCoverage {
                message: "process enumeration unavailable; no complete observed count".into(),
            });
        }
        let mut rows = mapping::sorted_rows(&latest.raw, latest.sequence, request.sort);
        let limit = request.limit.get() as usize;
        let truncated = latest.raw.display_truncated
            || latest.raw.process_totals.enumeration_truncated
            || rows.len() > limit;
        rows.truncate(limit);
        Ok(ProcessList {
            sample_seq: Count::saturating(latest.sequence), observed_at: mapping::observed_at(&latest.raw)?, rows: rows.try_into().map_err(mapping::invalid)?,
            observed_count: Count::saturating(latest.raw.process_totals.enumerated as u64), denied_count: Count::saturating(latest.raw.process_totals.access_denied as u64), truncated,
            note: Text::truncated("Bounded observation, not a complete ranking beyond the native display cap. Null means unavailable, denied or awaiting a second sample. Names may be display-truncated. Process GPU memory is not collected; shared working sets are never unique RAM totals."),
        })
    }

    /// Rule-based meaning and native unknown reasons for a currently observed process instance.
    /// PID reuse never reconnects a previous reference; no action is offered.
    pub fn processes_explain(
        &self,
        request: &ProcessRefRequest,
    ) -> EngineResult<ProcessExplanation> {
        let state = lock(&self.telemetry.shared.state)?;
        let latest = state.latest()?;
        let process = latest
            .raw
            .processes
            .iter()
            .find(|p| mapping::process_ref(p, latest.sequence) == request.process_ref)
            .ok_or_else(not_found)?;
        let mut facts = vec![];
        for (code, text) in [
            ("private_commit", "Private commit is memory promised backing by RAM or page files, not resident RAM."),
            ("private_working_set", "Private working set is resident private pages; null when EX2 is unsupported or observation fails."),
            ("working_set", "Working set includes shared resident pages; adding process rows double-counts shared pages."),
            ("cpu", "CPU fraction is CPU time over the monotonic interval divided by all logical CPUs; first-sample rates are null."),
            ("io", "I/O is bytes per second across all process devices, not just physical disk traffic; missing/regressed counters stay null."),
            ("identity", "Identity uses PID and exact creation FILETIME; unknown creation time is scoped to this observation only."),
            ("gpu_memory_unknown", "GPU Process Memory is not collected: PID-only counter instances cannot establish stable process identity. No process GPU sum is an adapter total."),
        ] {
            facts.push(ProcessExplanationFact { code: FactCode::new(code).map_err(mapping::invalid)?, text: Text::truncated(text) });
        }
        for unknown in process
            .unknowns
            .iter()
            .chain(&process.rates.unknowns)
            .take(17)
        {
            facts.push(ProcessExplanationFact {
                code: FactCode::new("observation_unknown").map_err(mapping::invalid)?,
                text: Text::truncated(&format!("{}: {}", unknown.field, unknown.reason)),
            });
        }
        Ok(ProcessExplanation {
            process_ref: request.process_ref.clone(), observed_at: mapping::observed_at(&latest.raw)?, facts: facts.try_into().map_err(mapping::invalid)?,
            summary: Text::truncated("Read-only process observation. Missing counters are unknown and learned or observed values grant no permission."),
            caveats: vec![Text::truncated("This is a retained sample, not proof the process is still alive. Observation failures may reflect denial, exit or missing support; they do not diagnose protection.")].try_into().map_err(mapping::invalid)?,
            available_actions: BoundedVec::default(),
        })
    }
}

#[cfg(test)]
mod tests;
