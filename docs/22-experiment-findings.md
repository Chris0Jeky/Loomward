# Initial reproducible experiments

Command: `python experiments/reference_smoke.py`. Evidence: `evidence/synthetic-experiments.json`. Seed: 20261009. All filenames, labels and scenarios are synthetic. Do not read the numbers as evidence about the owner's machine.

## Metadata scan smoke probe

The script creates 5,000 tiny files in a temporary directory, scans the same set three times and removes only that temporary fixture on exit. It asserts the expected file count and unqualified complete coverage. The recorded Linux/Python 3.13.5 run took approximately 1.006, 0.868 and 0.329 seconds, with a 0.868-second median. Files totalled 120,000 logical bytes.

This is an end-to-end reference scan smoke test, not a benchmark for cold NTFS, large directory hierarchies, network storage, persistent indexing, browser rendering or Rust. Files were newly created and cache state was not controlled. No WinDirStat/Everything/Czkawka comparison was performed. Use the full evaluation protocol before stating the product is faster or more efficient.

## Tier allocator against an exhaustive small oracle

The experiment enumerates each eligible tiny group being skipped or placed on each destination. It minimises remaining source-space shortfall under destination reserves and transfer budget. The test space is deliberately small; this exhaustive method is not a proposed large-scale runtime allocator.

Across 51 scenarios, the current greedy baseline never violated the checked transfer budget or destination reserve and never claimed executable proposals. However, it missed a feasible zero-shortfall target in nine cases and achieved greater shortfall than the oracle in 24. Maximum observed excess shortfall was 68 abstract byte units. This is a useful negative result, not a release-quality guarantee.

The preserved simple counterexample has a source with 10 free units, a goal of 30 and a transfer budget of 10. A colder group frees 10 at transfer cost 10; another eligible group frees 20 at the same cost. Coldest-first picks the first and cannot reach the goal. The exhaustive oracle selects the second. An allocator must weigh utility and resource constraints jointly, not merely order groups by coldness.

## Engineering consequence

Keep the greedy implementation as a deterministic baseline and golden fixture. LW-030 must add a better bounded solver and compare both feasibility and utility using this oracle. Options include exact branch-and-bound for small candidate sets and a bounded beam/knapsack-style heuristic for larger sets, with a deadline and explicit unfulfilled demand. Expanding search must not weaken eligibility, physical allocation accounting, path contracts or source-retirement requirements.

This experiment is narrower than the production objective. It does not model promotion, latency, churn, wear, changing free space, unknown heat, offline volumes during execution, crash recovery, backup fidelity or learned preferences. The wider experiment programme remains in docs/13-evaluation-performance.md and docs/18-opportunities-experiments.md.
