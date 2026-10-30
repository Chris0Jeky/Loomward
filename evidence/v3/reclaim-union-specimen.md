# Synthetic reclaim inspection - NOT AN ACTION PLAN

All labels and byte quantities are fictional. No scan or file operation occurred.

**Permission:** none; inspection is not removal approval.

**Hypothesis:** All selected names cease referencing their objects; no retained open handles, snapshots or shared extents. Recycling or renaming alone gives zero immediate relief.

**Target status:** proved_in_static_model

| Volume | Requested relief | Selected logical | Selected allocation | Guaranteed relief | Possible relief |
|---|---:|---:|---:|---:|---:|
| archive | 0 | 0 | 0 | 0 | 0 |
| fast | 16 | 160 | 16 | 16 | 16 |

Selected: cache, project.

Search: 71/50000 nodes; budget exhausted: False; optimal in this model: True.
Minimum guaranteed-shortfall bound: [0, 0] bytes. minimum total per-volume guaranteed shortfall; count/logical tie-breaks unbounded until complete.

## Choices and exclusions

| Choice | Selected | Individual verified relief | Explanation |
|---|---|---:|---|
| project | True | unknown | included: removing this choice increases guaranteed shortfall by 16 bytes; Fictional build tree; rebuild cost unknown |
| build | False | unknown | ancestry conflict with project; Synthetic candidate; disposability and rebuild cost unknown |
| cache | True | unknown | included: removing this choice increases guaranteed shortfall by 12 bytes; Fictional cache sharing the build object |
| standalone | False | 7 | not selected by the exact shortfall/count/logical/ID objective; Synthetic candidate; disposability and rebuild cost unknown |
| mystery | False | unknown | not selected by the exact shortfall/count/logical/ID objective; Synthetic candidate; disposability and rebuild cost unknown |
| unmeasured | False | unknown | not selected by the exact shortfall/count/logical/ID objective; Synthetic candidate; disposability and rebuild cost unknown |
| pinned | False | unknown | pinned; outside selectable set; Synthetic candidate; disposability and rebuild cost unknown |
| other-volume | False | 50 | no requested relief on this volume; Synthetic candidate; disposability and rebuild cost unknown |

## Object facts and unknowns

Objects below are counted once per (volume, file identity). Observed names may occur in nested choices.

| Volume / identity | Observed names | Total links | Logical | Allocation | Resident | Facts / unknowns |
|---|---|---:|---:|---:|---|---|
| archive / shared | archive-link | 1 | 500 | 50 | True | input allocation and link count known |
| fast / extra | extra-link | 1 | 40 | 4 | True | input allocation and link count known |
| fast / mystery | mystery-link | unknown | 80 | 8 | True | outside links unknown |
| fast / pinned | pinned-link | 1 | 90 | 9 | True | input allocation and link count known |
| fast / shared | build-link, cache-link | 2 | 120 | 12 | True | input allocation and link count known |
| fast / single | single-link | 1 | 200 | 7 | True | input allocation and link count known |
| fast / unmeasured | unknown-link | 1 | 1 | unknown | None | allocation unknown; logical size is not an upper bound; residency unknown |

## Ranking comparison

Current reclaim deliberately supplies an individual ranking and does not sum nested candidates. It has no set-selection claim.

A hypothetical antichain-aware prefix of that within-group ranking chooses cache, project, standalone; objective [0, 3, 360].
This set search chooses cache, project; objective [0, 2, 160] (guaranteed shortfall, choice count, unique logical bytes).

Logical size is not relief. Unknowns are not zeros: zero is a conservative lower bound. A possible upper bound is not an achievable promise.

No native identity/freshness, actual free-space increase, durability, recoverability, semantic disposability or owner approval is proved.
