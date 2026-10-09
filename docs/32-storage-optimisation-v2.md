# Storage optimisation v2: a bounded search, not a universal allocator

## The original failure is retained

The first pass's greedy planner sorted eligible groups primarily by coldness. Its tiny exhaustive experiment found nine missed feasible targets among 51 cases. A cold small group could spend the whole transfer budget while a slightly warmer eligible group would have met the target. The original implementation, experiment and result remain unchanged for comparison.

The new allocator is in `python/loomward/planner_v2.py`. The live UI calls `/api/plan-v2`; the original `/api/plan` and Rust baseline are preserved. The standalone HTML uses the clearly labelled greedy preview, not an untested JavaScript port of the bounded search.

## Objective and constraints

For supplied disjoint groups, minimise lexicographically: unmet free-space target, transfer bytes, number of selected groups, source-byte-weighted heat, then stable group/target IDs. Constraints include target reserves, transfer budget, source membership, online/writable status, pinning, protection, activity and cooldown/history.

Keep source relief, target capacity requirement and transfer bytes separate. Compression, sparse allocation, replicas and metadata can make them differ in a real implementation. The reference only consumes caller estimates; it does not measure those relationships.

Hard exclusions are not softened by a better objective score. Unknown heat/history stays ineligible under this policy. A request cannot spend more capacity because the model says it is confident. The source and target must later be revalidated natively before any effect.

## Search strategy

First run the old planner as a feasible incumbent. Add deterministic portfolio candidates ordered by relief-per-transfer, larger relief, and lower transfer cost, with a best-fit target heuristic. For at most 12 eligible groups and four eligible targets, run a node-bounded enumeration with optimistic relief pruning and incumbent cost bounds.

The search stops when the configured node budget is exhausted. It returns the best feasible incumbent and says that optimality is not proved. Larger problems use the portfolio only and report the size limit. Reaching zero shortfall proves that component cannot improve below zero; it does not prove minimum transfer or overall lexicographic optimality unless the search completed.

The current HTTP/MCP simulations use a 5,000-node budget. The direct function default is 50,000 and accepts at most 200,000. Those are explicit reference-work bounds, not promises of a particular runtime on every machine.

## Evidence

`experiments/v2_benchmarks.py` reproduces the exact seed and 51 old scenarios. All new searches complete within the configured budget in that experiment. The old allocator misses nine feasible targets and has positive shortfall regret in 24 cases; the new allocator has zero of each against the tiny oracle. No checked transfer/reserve constraint is violated.

These are abstract byte-unit cases. They do not model crash safety, source changes, overlapping groups, encrypted streams, reparse points, physical allocation or future use. “Optimal” in output is explicitly scoped to the static supplied model. The result does not justify unattended movement.

## Bidirectional placement remains a separate programme

Evicting cold groups is only half of the desired behaviour. Recall needs a demand forecast, destination availability, a latency budget and path continuity. A project about to be used may need prefetch; a rarely used but hard-to-recover item may still deserve protected fast residency.

A future solver should operate on a rolling horizon and charge movement in both directions. Add minimum residence time, cool-down and a limit on repeated migrations. Use explicit active-project/model leases before inferring use from weak timestamps. Do not let two independently running placement policies move the same group back and forth.

Possible next algorithms include a bounded dynamic programme for a single target, a mixed-integer formulation in an offline oracle, and a beam search for larger grouped cases. The operational solver should retain a hard work budget and always return a feasible incumbent or an explicit infeasible/unknown result. An external optimiser is not required for the first native release.

## Recovery and physical relief

A same-source quarantine consumes the source capacity it is supposed to release. The product must explain the trade-off between keeping original bytes, creating an independent recovery copy and actually retiring the source. Do not report “space freed” while merely relabelling a directory.

Before source retirement, validate the copied data, metadata fidelity, destination durability and the selected recovery policy. A shortcut or junction can preserve a name in some circumstances but is not a universal compatibility layer. Offline targets, application path assumptions and permission differences require explicit handling.

The result view should display estimated versus observed relief separately and retain why a group was excluded. A failed recall is a high-cost outcome that must influence later policy evaluation; do not hide it under a successful transfer counter.

## Porting gate

The Rust planner is still the original baseline and is uncompiled in this environment. Port v2 only after native compilation and with the same scenario/output fixtures, including cutoff and no-optimality cases. Do not change the original fixture contract silently. Native integer types, overflow, UTF-16 names and volume identity require their own tests rather than assuming Python semantics transfer directly.
