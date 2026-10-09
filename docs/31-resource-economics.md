# Resource economics and cooperative admission

## Control the work before trying to control the operating system

Loomward should first budget its own scans, hashes, extractors and models. Only then should it coordinate with voluntary external workers. An app that constantly loads a large model to manage memory has failed its own purpose.

The reference implements pure vector admission and an in-process lease broker. It does not start a process, inspect a PID, adjust a priority, call a memory-trimming API or enforce a quota. Its outputs consistently say simulation or non-executing reservation.

## Demand vectors

A request estimates incremental CPU slots, memory MiB, GPU memory MiB and I/O MiB/s. Production requests should add per-volume I/O, scratch-space needs, network budget, power/thermal sensitivity and cancellation cost only when a provider can supply meaningful estimates. More dimensions are not automatically more accurate.

A job fits only when every demanded dimension fits the remaining budget. Unknown capacity blocks positive demand in that dimension. A zero-demand dimension does not block a job merely because its capacity is unknown. Telemetry older than 30 seconds defers work in the reference; this is a design threshold, not a universal freshness law.

Admission order uses supplied priority plus bounded age credit, with deterministic tie-breaking. This is not globally optimal scheduling or a starvation-freedom proof. A job larger than every admissible budget must become visibly unschedulable or ask for a different plan, rather than wait indefinitely with a reassuring spinner.

## Avoid double-counting

Distinguish observed use from reservations. Suppose measured available memory already reflects a running model. Subtracting that model's full reservation again double-counts it. The production coordinator needs a lifecycle that distinguishes not-yet-started demand, observed running consumption, peak safety margin and released demand.

The current simulator assumes the caller supplies a capacity basis and already-counted reservations consistently. It subtracts the supplied reserved vector once. It cannot infer whether an external memory measurement includes a particular job. The UI therefore uses clearly synthetic estimates, not a misleading hybrid of measured host memory and invented reservations.

For a real adapter, document the accounting basis per dimension and track which reservation has transitioned into observed consumption. Prefer conservative headroom when matching is uncertain, but report the reason so the owner can diagnose apparently unused capacity.

## Lease semantics

A cooperative lease reserves a vector for a bounded duration. The reference accepts TTLs from 5 to 3,600 seconds, has a monotonic clock, serialises competing admissions under a lock and returns an opaque release token. Twenty simultaneous one-slot requests against a one-slot budget yield one admitted lease in the test.

Idempotency is scoped to owner plus request ID and an exact demand/TTL fingerprint. Replaying a request does not extend its expiry. Reusing the key for different inputs is an error. An expired or released request does not resurrect on retry. A denied request requires a new request identity before a fresh admission attempt.

The owner string is self-declared, not authenticated. The release token is only a capability over this ephemeral reference reservation. The broker is not exposed through MCP or HTTP. Its state is not durable and its epoch changes on restart. Production use needs authenticated principals, restart reconciliation, bounded durable history, renewal semantics and proof that a worker cooperates.

## Owned-worker enforcement

For workers Loomward actually launches, Windows Job Objects provide a documented mechanism for grouping processes and applying limits, but job membership, nesting and breakaway rules matter [V18]. The application must check each operation's outcome rather than assume a policy was enforced because an API call was attempted.

Quality-of-service choices can express performance/power intent on Windows; they are not a promise of a specific memory reduction [V12]. Any future UI should distinguish advisory priority, cooperative pausing and enforced constraints. Do not describe a best-effort signal as a hard allocation.

Working-set trimming is not the primary strategy. A smaller resident set can produce more paging and worse experience. The desired outcome is a faster foreground task and bounded background work, not a cosmetically low RAM number. Keep a direct expert escape hatch to the system's normal tools rather than attempting to manage every service.

## Hysteresis and anti-oscillation

A coordinator should not alternate every few seconds between admitting and pausing the same worker. Use separate admission and recovery thresholds, minimum residence times, cooldowns and a budget for reversals. Resource-pressure sampling should have smoothing with visible lag, not hide uncertainty behind a stable but stale gauge.

External controllers complicate this. A model host may unload a model independently; Estate Console may pause a lane; a backup tool may throttle itself. Establish one owner of each effect. Loomward can request or advise, but it must not countermand another controller without an explicit integration contract.

## Resource-aware model routing

Maintain a small always-available student and load the teacher only when expected decision value justifies its cost. Coalesce related questions, cap prompt/context size, deduplicate identical feature batches by model and consent revision, and unload a model that Loomward owns when the selected policy permits it. Do not unload a model used by an unrelated process.

GPU VRAM, system memory and disk residency are coupled. Loading a model can increase RAM pressure while moving its files creates I/O. A future joint planner should reserve all stages, including scratch space and warm-up, instead of admitting each stage independently and deadlocking halfway through.

## Evaluation

Replay identical workload arrivals under independent workers, fixed quotas and cooperative admission. Measure foreground latency, completed useful work, peak pressure, queue age, cancellation delay and Loomward's own resource cost. Track demand-estimation error separately. A simulation using perfect job demands cannot demonstrate reliable admission under real peaks.

The supplied scheduler and lease tests establish reference invariants and failure behaviour. They do not show that a Windows workstation is faster, quieter or more energy-efficient with Loomward installed.

## Source references

- [V12] Windows quality of service: https://learn.microsoft.com/en-us/windows/win32/procthread/quality-of-service
- [V18] Windows Job Objects: https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
