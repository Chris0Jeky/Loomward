# Evaluation, performance and release gates

## Three kinds of evidence
**Functional evidence:** code produces correct results on defined fixtures, rejects invalid input and obeys capability limits. **Platform evidence:** Windows-specific filesystems, APIs, permissions, metadata, process states and WebView2 behave as expected. **Product evidence:** users find files faster, need fewer corrections and experience less interruption. One kind is not a substitute for another.

The Python tests and Chromium renderer checks establish reference behavior in the authoring environment. The Rust source tests, Windows cases and comparison benchmarks remain unexecuted until a suitable host runs them. CI configuration is not evidence that CI passed.

## Proposed budgets, not measured promises
| Metric | Initial engineering target | Required measurement |
|---|---|---|
| Warm app-visible start | Under 2 seconds without model load | p50/p95 on supported Windows hardware |
| Indexed lookup | p95 under 100 ms on a 1M-item local catalogue | Query mix, index size, cold/warm storage |
| Idle engine CPU | Under 0.5% total-machine CPU averaged over 10 min | Includes timers and watchers |
| Idle engine private commit | Under 80 MiB without content/ML workers | Separate from shared/resident pages |
| Idle UI plus engine footprint | Under 250 MiB resident in a stated test | Include WebView child processes |
| User cancel acknowledgement | Under 250 ms at UI/API layer | Native operation cancellation may finish later |
| Optional teacher activity | One request in flight initially | Track total model-server RAM/VRAM and load latency |
| Background content work | User-budgeted; back off on contention | Foreground latency and device throughput |
| Data-loss incidents | Zero acceptable | Fault injection plus staged field exposure |

Targets are hypotheses to refine, not guaranteed requirements met by the reference. The prototype is intentionally bounded at far less than a million rows and does not implement the permanent incremental catalogue.

## Storage correctness corpus
Use empty and deeply nested directories, millions of small entries, very large sparse files, long paths, unusual Unicode, hard links, symlinks/junctions, case-sensitive subdirectories, ACL-denied entries, compressed/EFS files, alternate streams, cloud placeholders, open/locked files, network roots, removable volumes and deliberate drive remapping. Some cases require dedicated support or explicit refusal. Record skipped/unsupported counts and verify that they do not disappear from aggregate coverage.

Compare logical, per-entry allocation, unique-object allocation, OS-reported volume use and unknown/unattributed bytes. Ensure hard links are not counted as multiple independent physical savings. Duplicate testing must include same size/different bytes, changed files between scan and hash, link replacement, budget exhaustion and metadata differences.

## Fair comparator methodology
Use current verified builds of WinDirStat [R01], an appropriate filename index such as Everything [R04], Czkawka [R02] for duplicate work and System Informer [R03] for process observability. Record exact versions, OS build, security software, device models/topology, filesystem, index freshness, privileges and datasets. Explain that the tools have different feature coverage.

Separate initial scan, warm rescan, incremental catch-up, query latency, duplicate-content throughput and UI rendering. Equalise exclusions and access rights. Report p50/p95, memory, I/O, CPU and foreground interference. Avoid comparing a cached index with another tool's cold full scan and then claiming a universal speedup. No WinDirStat performance comparison has been executed in this package.

## ML evaluation
Retain a human-labelled dataset with time/group-disjoint holdouts and explicit provenance. Do not report synthetic-fixture accuracy as user preference accuracy. Report a majority/extension baseline, the current student and each proposed richer representation on identical splits. Measure per-class errors, top-k utility, selective error versus coverage, OOD behavior, human correction burden and model footprint.

Test teacher failure modes separately: invalid JSON, duplicate fields, wrong item ID, unsupported label, instruction-bearing filename, extreme response size, HTTP redirect, network timeout and absent consent. A schema-conforming teacher can still be wrong. Calibration experiments [R31] only measure prediction reliability for their evaluation distribution, never permission correctness.

## Tier-planner evaluation
Build a small exhaustive oracle for synthetic cases to measure when greedy planning misses a feasible or lower-cost arrangement. Verify all hard constraints independently of the solver. Report source benefit, destination allocation, transfer cost, cooling/pin compliance, residual shortfall and churn. A heuristic may be acceptable if it is transparent, feasible and fast; never label it optimal without proof.

Longer simulations should include unpredictable foreground work, reconnecting USB drives, capacity growth, unknown heat, sequential versus random I/O and backup failures. Measure promotion/demotion oscillation and user's wait time, not merely bytes moved. Production capacity schedules must include intermediate staging and retention space.

## Windows mutation gate
No executor ships without native handle-identity tests, metadata-preservation fixtures, locked/cloud/protected-file refusal, stage-by-stage crash recovery, cancellation, volume remap handling, independent restore evidence, approval digest tests and a review of the privilege boundary. Run first on disposable data in a VM and then a dedicated test tree. Do not treat a successful move of two text files as sufficient validation.

## Evidence storage
Keep exact commands, environment summary, stdout/stderr, fixture seeds, commit/build identity, checksums and result summaries. Distinguish PASS, FAIL, SKIP and UNVERIFIED. Retain regression failures that motivated fixes, but do not mistake those red logs for current suite failures. `evidence/` records this pass; `handoff/WINDOWS-VALIDATION.md` defines the next platform pass.
