# Catalogue performance hypotheses — 17 October 2026

KRASPYON laptop, Windows 11 Home, 18 threads, 16 GB RAM, shared with approximately ten
agent lanes. Release binaries, `CARGO_BUILD_JOBS=4`, private target directory
`C:/lw/target/perf-catalog`. Synthetic metadata only; no enumeration, native file effects,
personal names, or real inventories. Refs #148 and #386; neither issue is closed here.

## Method

The new [`catalog_perf` example](../../crates/loomward-catalog/examples/catalog_perf.rs)
reuses the catalogue examples' first-scan bulk writer and the lab's 100k/1M size tiers.
Each fresh temporary catalogue has 100,000 or 1,000,000 files, 1,000 per directory,
with native-style synthetic identities. A fixed second root has 200 files under 100 parents.
All data passes through the real writer; reads use the real `Reader`. No new dependency.

Baseline is `879c6ce2656928587219c3f25da15dbaeed288f2`, compiled with this same example.
Only the two edited implementation files were temporarily replaced with their HEAD bytes for
that build, then restored byte-for-byte before building the candidate. Frozen baseline binary
and raw receipts remain in ignored `.loomward/catalog-perf/`. The four final receipts are
preserved in [aggregate JSON](bench/catalog-perf-2026-10-17.json).

Run from this worktree, with the environment above:

```powershell
cargo build --release -p loomward-catalog --example catalog_perf
C:/lw/target/perf-catalog/release/examples/catalog_perf.exe 100000
C:/lw/target/perf-catalog/release/examples/catalog_perf.exe 1000000
```

Seven repetitions per read/refresh/control, median wall milliseconds below; bulk build and
forget are single observations in seconds. Reader timings include serialization for search
and slice, as in the existing catalogue benchmarks. Cache is warm/uncontrolled, not cleared.
Both sizes finished. No quiet-host or end-to-end service throughput claim is made.

## Before and after

| Path | Before 100k | After 100k | Before 1M | After 1M |
| --- | ---: | ---: | ---: | ---: |
| First scan through EndRun, seconds | 1.705 | 2.371 | 21.464 | 22.658 |
| Refresh the same 1,000-file directory, ms | 26.67 | 26.90 | 16.75 | 26.78 |
| Search: 100 hits in distinct parents, ms | 2.49 | 2.47 | 2.08 | 1.35 |
| Slice: 100 nodes, ms | 3.38 | 1.16 | 4.02 | 1.09 |
| Slice: 1,000 nodes, ms | 32.70 | 9.97 | 36.22 | 7.92 |
| Slice: 6,000 nodes, ms | 209.83 | 49.97 | 213.50 | 44.45 |
| Duplicates: 2 candidates from fixed root, ms | 3.65 | 3.54 | 6.48 | 7.95 |
| Duplicates: 200 candidates from fixed root, ms | 30.51 | 21.99 | 34.82 | 28.58 |
| Forget the same 200-file root, seconds | 0.476 | 0.785 | 4.290 | 19.181 |
| SQL control: delete its 200 rows + FTS optimize + rollback, ms | 121.93 | 133.11 | 1,084.16 | 1,636.20 |

The 6,000-node slice improved **4.20× / 4.80×** at 100k/1M. Its before/after ranges were
171.23–238.70 / 44.50–51.04 ms at 100k and 171.89–290.43 / 39.34–50.96 ms at 1M.
Duplicate 200-candidate medians improved **1.39× / 1.22×**; ranges overlap, so this is a
modest shared-host observation, supported by the deterministic reduction in SQL below.
The unmodified build, refresh and forget timings expose substantial host variation; they
must not be interpreted as effects of reader-only changes.

The duplicate query against the *large* root exhausted `ResourceBudget` on all seven
attempts at both sizes and both revisions. Those are refusals, not completed-query timings.
Its existing two-million-instruction bound is retained. Optimizing candidate path assembly
does not solve the size-grouping query's catalogue work.

## Decisions on the five hypotheses

1. **Forget: scaling confirmed, retained for erasure correctness.** The SQL control grows
   roughly ninefold for a tenfold larger unrelated catalogue. The public command also owes
   whole-store vacuuming and WAL cuts (`scrub_pending`/`wal_cut`, #391), so replacing the
   optimize alone would not make complete forgetting proportional to the removed root.
   [SQLite's contentless-delete semantics](https://www.sqlite.org/fts5.html#the_deletemerge_configuration_option)
   leave deleted terms in segment trees until merging discards them. A bounded
   [merge](https://www.sqlite.org/fts5.html#the_merge_command) can stop before that happens;
   completing incremental merges performs the same total optimization work. The current
   physical-name-removal tests and crash recovery contract require that scrub debt survives
   until it is paid. A drop-in bounded merge could let the existing scrub marker clear while
   deleted terms remain in live FTS segments; making that resumable needs a separate design.
   No FTS settings, persistence protocol, or erasure guarantee changed.
2. **Bulk identity matching: rejected as a first-scan diagnosis.** `publish` already calls
   `match_file` only when `has_old` is true; a first bulk listing has no old files. Prepared
   continuity also supplies precomputed matches. The fixed-size refresh does not demonstrate
   a catalogue-size-dependent identity matching bottleneck. Writer code stays unchanged.
   This does not settle performance of a very large single-directory rescan, P4's stretch
   target, or #386's bulk-page footprint.
3. **Search ancestor walks: no fix justified.** `place` already caches every visited ancestor
   for the request. One hundred hits across distinct parents remain around 1–3 ms as total
   catalogue size grows tenfold. This fixture does not cover very deep unique ancestor chains.
4. **Slice threads: fixed.** Repeated root-volume/tier SQL was proportional to output nodes.
   A request-local cache keyed by `(root, coverage, flags)` reuses identical `Threads` while
   preserving each node's permission state. It lives inside the same read transaction and
   cannot retain stale tier declarations across requests. No protocol or ordering changes.
5. **Duplicate paths: fixed within the bounded candidate phase.** The candidate SELECT already
   knows each file's name and parent; it now returns them instead of rereading the file.
   Parent `NodePath`s are cached for the request. The file still consumes one of the original
   512 path slots; path-length, serialized-byte, singleton, exclusion and visibility limits
   are preserved. This is bounded by the existing 200-candidate cap.

## Regression evidence

Before implementation, the slice regression failed with 100 tier-query preparations instead
of four. Afterward it compares serialized thread bytes for distinct roots, coverage states,
and flags, with four tier queries for four distinct tuples. SQLite's authorizer counted
204 file-name read authorizations during SQL preparation in the duplicate regression;
afterward it permits at most four and compares every returned path byte-for-byte with
`Reader::path`. These are SQL preparation counts, not physical disk-read counts. Existing
nested-path and over-depth tests remain in place.
Search hit payload sizes and all six slice payload sizes match their corresponding baseline
receipts exactly; counts alone are supplementary evidence, not a substitute for the tests.

## Verification

Run in `C:/lw/wt/perf-catalog` with the target directory and four-job limit above.

| Command | Windows result |
| --- | --- |
| `cargo fmt --all --check` | PASS after correcting the new test's formatting |
| `cargo clippy -p loomward-catalog --all-targets --all-features -- -D warnings` | PASS |
| `cargo test -p loomward-catalog --all-features` | 229 passed, 0 failed, including 9 default crash-sweep tests |
| `cargo test -p loomward-catalog --features test-hooks --test crash_sweep` | 9 passed, 0 failed; `LOOMWARD_CRASH_SWEEP` unset (default mode) |
| `cargo test -p loomward-service` | 200 passed, 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace --no-fail-fast` | 1,158 passed, 1 failed, 0 ignored; exit 101 (existing service TTL test below) |
| `cargo test -p loomward-service --lib idem::tests::live_slots_are_capped_and_never_evicted_early -- --exact` | 1 passed, 0 failed on the single isolated rerun |
| `git diff --check` | PASS |

The workspace failure is in unchanged `crates/loomward-service/src/idem.rs:182`:
`live_slots_are_capped_and_never_evicted_early` expected `Full` but received `Run` at
"unexpired outcome kept". This test gives completed outcomes a **50 ms real-time TTL**.
`claim_in` removes an outcome only when its elapsed time reaches that TTL, so the completed
slot expired between `finish` and this assertion. This cache code has no catalogue calls;
the failure is outside the reader changes. The earlier entire service run passed all 200
tests, and the one isolated rerun passed. The failed workspace run is retained, not replaced
by the isolated result; no service code or test was changed. Noticed for a separate lane:
make this test's expiry transition deterministic instead of requiring scheduling within 50 ms.

The five Io(50) host failures named in the brief did **not** reproduce: the engine's four
native identity/containment tests and the Windows rename/reopen test all passed in this run.
No fix for #292 is claimed. Raw gate logs remain in ignored `.loomward/catalog-perf/`.

## NOT verified and residual risk

No quiet-host A/B, cold-cache I/O, 10M catalogue, end-to-end HTTP scan, or large single-directory
rescan measurement. The large-root duplicate grouping query still hits its work budget;
forget still scales with the whole catalogue to satisfy its current scrub contract. Timing
ranges are observations on a busy laptop, not latency guarantees. No commit, push, PR, merge,
release, or safety-gate change is performed by this peer lane.

The workspace is **not fully green** because of the service TTL assertion above. All scoped
catalogue checks, both explicit catalogue/crash commands, and the full service command passed.

Human actions remain in [`HUMAN_TODO.md`](../../HUMAN_TODO.md), unchanged by this work.
