# Storage tiering and capacity planning

## The real problem
A filesystem balancer is not “large files to D:.” It is a constrained placement problem over groups, uncertain future use, storage capacity, transfer cost, availability and application path dependencies. The performance of an NVMe disk, SATA SSD, HDD, USB enclosure or network volume depends on more than a label. Measure capabilities or let the user declare them; do not infer speed from a drive letter.

Keep `source_bytes_relieved`, `destination_bytes_required` and `transfer_bytes` separate. Sparse files, compression, hard links, deduplication, alternate streams and provider behavior can make these differ. A destination estimate can include staging and metadata overhead. A source-space benefit may not occur until retained originals are safely removed. The prototype therefore simulates user-supplied group estimates and never calls its result measured space reclaimed.

## Placement model
Let x[g,d] indicate group g assigned to device d. Feasible placement satisfies capacity after reserve, eligibility, mandatory replicas, availability, filesystem capabilities, path/application compatibility and user pins. A proposed objective is:

`expected_access_cost + transfer_cost + foreground_interference + uncertainty_penalty + churn_penalty + energy/wear_cost`

subject to the non-negotiable constraints. Weights express preferences, not permission. The LLM can help the user choose an objective or explain alternatives; it cannot rewrite the constraints. Small exact optimisation or MILP/min-cost approaches are candidates after an interpretable baseline, not mandatory dependencies for the first release.

Capacity constraints apply to intermediate as well as final states. Copying may require both original and staged destination to exist. Backups may need a third copy or repository headroom. A feasible final diagram is not a feasible transaction schedule if the intermediate destination fills up.

## Heat and latency
Represent heat as expected next-use likelihood over a stated horizon, with confidence and coverage. A frequently streamed media file may be fine on an HDD while a small randomly accessed project benefits strongly from SSD latency. Restore cost includes spin-up, enumeration, transfer, application restart, unavailable devices and user disruption. Unknown heat is not cold.

A group includes all required dependencies and has an activity/pin status. For a model bundle, include tokenizer/config/support files and the fact that weights may be mapped by a running model server. For a code project, preserve relative paths, worktrees and application references; do not independently tier `.git` files or dependency internals. A dedicated provider can later distinguish recreatable caches from valuable state.

## Reference solver
`plan_tiers` takes at most 32 volumes and 10,000 disjoint groups. It validates safe integer byte fields, unique IDs, free/reserve bounds, source allocation totals, heat in [0,1] or unknown, and history. Eligibility requires explicit false for pinned/active/protected, known cold heat at or below 0.25, known residence beyond the default seven-day cooldown, and an online writable source.

Eligible groups are ordered by increasing heat, decreasing source benefit and stable ID. Candidates go only to an online writable same/slower tier that preserves its reserve. The reference chooses the slowest qualifying destination, then free space, then ID. Transfer budget is respected; it stops when the source target is met or candidates run out. It returns rejections, projected capacities, residual shortfall, explicit non-executable proposals and no optimality claim.

This first solver only demonstrates **demotion under pressure**. Promotion back to faster storage, multi-source balancing, learned heat, physical allocation discovery and application-aware path continuity are designed here but not implemented. A synthetic scenario is not a live map of the user's disks.

## Production controller
Use hysteresis and residence rules to avoid ping-pong. A candidate initial policy could enter pressure handling below the larger of a configured minimum reserve and 15% free, and aim above a distinct 22% watermark. These are starting hypotheses, not universal recommended disk thresholds. Expose exact GiB and percentage values and let the user choose. Do not run a benchmark or generate write traffic on a disk without explicit permission.

Enforce per-device and daily transfer budgets, concurrency limits, bandwidth/latency feedback, cooldowns, foreground-session pause, battery/thermal policies and quiet hours. Re-evaluate before each group, not only when the nightly plan was generated. Count hidden load from hashing, backup, antivirus and cloud sync; a large copy can interfere with all of them.

Capacity forecasts should begin with a rolling growth baseline and uncertainty bands. A projected emergency asks for attention earlier rather than silently widening permission. Distinguish source pressure from an unrelated destination whose reserve is already violated. Never propose moving back to a fast disk just because yesterday's demotion made it temporarily spacious.

## Bidirectional access and path continuity
Semantic collections should resolve logical item IDs to current ordinary paths. That supports in-app recall without pretending every external application can survive a path change. Transparent hydration/virtualisation through a filesystem filter or Cloud Files provider is a substantial separate project with integrity, application-compatibility and driver/provider lifecycle costs. Do not hide those costs in an MVP.

For user-owned ordinary documents, offer explicit moves and managed shortcut updates after approval. For applications with configured library locations, use a provider and the app's supported location mechanism. A junction or symlink is not a universal solution: it changes semantics, can break installers/backup tools and affects the safety boundary [R09,R10]. Keep it an advanced opt-in provider feature with dedicated tests.

Promotion is triggered by an explicit working-set pin, planned session or evidence-backed forecast. Stage rather than immediately evict another working set, show time/space cost, and allow cancellation. If a removable source is absent, retain an unavailable reference with the device name; do not substitute a different file with the same name.

## User-facing plan options
Show at least: no change; minimal-transfer plan; maximum short-term relief within budget; keep all current projects pinned; and archive completed groups only. Plans explain what cannot be achieved and why. Every eligible plan must already satisfy safety constraints before preferences rank it. “Would you prefer 36 GiB reclaimed with one completed project, or 50 GiB with that project plus an old model bundle?” is useful; “Which system folders may I delete?” is not an appropriate escape from infeasibility.
