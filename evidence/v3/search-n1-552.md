# #552: measured search query costs

Synthetic SQLite queries on KRASPYON, Windows 11 Home (10.0.26100), rustc/cargo
1.97.1, **debug** build, `CARGO_BUILD_JOBS=4`, lane-owned target directory.
Baseline: `4babf67`, with the new probe only. Comparison: this working-tree patch.
At closeout `origin/main` had advanced to `3ad1870`; its changes since the baseline
touch neither `search.rs` nor `freshness.rs`. Integration on that newer base remains
with the driver; this lane does not rebase or commit.
These are query-cost measurements, not release-build latency, scanning, cold-cache,
HTTP, watcher-delivery, or real-inventory evidence.

## Scope and prior fixes

Read issue #552 and its comment before changing code. Its three search hypotheses
are in scope; the later writer/developer-clutter hypotheses are separate, unmeasured work.

* The 128-candidate FTS restart loop was still present. A regression fixture with
  512 out-of-scope hits before one eligible hit measured **5 statement executions**;
  the same fixture now measures **1**, with the same eligible hit and watermark.
  Each kind/pass now streams until the existing page limit or work/deadline guard stops it.
* Search already memoized shared parents. The remaining cost was uncached ancestry:
  100 disjoint depth-20 branches require 2,001 point lookups with that cache. Search
  now uses one recursive query per uncached parent, stopping at a cached ancestor.
  A depth-30 regression measures **30 executions before / 1 after** and checks path
  text, cached reuse, hidden ancestry, and refusal of missing ancestry.
* Refresh still resolves path components individually. Measurement confirmed repeated
  SQL preparation overhead; reuse the existing lookup statement. No materialized
  path index or metadata cache is added. The 1,024 depth-20 path probe still performs
  **21,504 SQL calls**; only child-statement preparation is reused. A regression
  checks 60 component executions on the cached statement, containment through
  `excluded`/`absent_pending`, invalid UTF-16, and an unknown root.
* **Already fixed by `b28f266` / PR #575:** cyclic/over-depth ancestry no longer
  aborts the entire search page. Such hits are omitted and coverage is partial,
  preserving visibility checks; the existing `a_cyclic_hit_does_not_fail_other_search_hits`
  exercises both plain and ranked search. This patch preserves that behavior rather
  than returning a location whose hidden ancestors have not been checked.

## Reproduction

`search_n1` builds generated catalogue rows directly through SQLite with the normal
trigram triggers, and registers two synthetic roots through the writer. It creates
1M or 10M `common-<integer>.txt` files in one root plus 100 `deep-<integer>.txt`
files under disjoint depth-20 branches in the second. It uses no filesystem scan,
personal metadata, or ranked-token index. Reuse each fixture for before/after runs.
The 10M baseline fixture took 312.169 seconds to build; normal integrity checking on
reopen is outside query timers. Each request is repeated three times, including its
first run. This is a small diagnostic sample on a shared host, not a stable p95.

```powershell
$env:CARGO_BUILD_JOBS = '4'
# CARGO_TARGET_DIR stays the exported C:\lw\target\lane-search-552 throughout.
cargo run --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-catalog --example search_n1 -- C:/lw/wt/search-552/.loomward/search-n1-1m 1000000
cargo run --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-catalog --example search_n1 -- C:/lw/wt/search-552/.loomward/search-n1-10m 10000000
```

The common-token requests use `limit=100`, `kind=file`, a 50,000,000 VM-operation
budget and no wall-clock budget. The narrow scope permits only the other root:
zero hits and a budget-cut continuation are expected, not successful completion of
the entire query. A faster budget cut need not have examined the same number of
candidates; do not treat its latency ratio as a fixed-work throughput ratio.
The ordinary owner request returns 100 hits; the ancestry request returns all 100
deep hits. The resolver compares the exact service SQL separately, without the
service's watcher or scan scheduling. The final probe also prints the cursor key,
paired prepared-resolver timing, and progress; these additions do not change the
three search requests or fixture.

## Results

Median of three search calls, milliseconds (first run included):

| Fixture | Query | Before | After |
| --- | --- | ---: | ---: |
| 1M | Owner common-token page | 6.846 | 5.359 |
| 1M | Narrow-scope common-token budget cut | 3,304.111 | 1,238.727 |
| 1M | 100 hits under disjoint depth-20 branches | 22.322 | 19.068 |
| 10M | Owner common-token page | 6.033 | 3.409 |
| 10M | Narrow-scope common-token budget cut | 5,030.590 | 855.937 |
| 10M | 100 hits under disjoint depth-20 branches | 28.153 | 16.312 |

Within the comparison invocation, the paired 1,024-path SQL probe measured
469.800 ms uncached / 185.569 ms prepared on 1M, and 526.202 ms / 183.768 ms on
10M. These are single paired runs; the baseline invocations separately measured
474.312 ms and 641.874 ms for the uncached version.

Raw receipts: [before 1M](bench/search-n1-552-before-1m.json),
[after 1M](bench/search-n1-552-after-1m.json),
[before 10M](bench/search-n1-552-before-10m.json),
[after 10M](bench/search-n1-552-after-10m.json).
The two owned synthetic fixture directories were removed after the receipts were
captured; no target-directory content was removed.

## Verification

Three new regression tests were run before their respective fixes:

* `cargo test --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-catalog --lib a_scoped_pass_streams_past_rejected_hits_without_restarting_fts`:
  red (5 executions, expected 1), then green.
* `cargo test --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-catalog --lib an_uncached_search_parent_uses_one_ancestor_query_and_keeps_visibility`:
  red (30 executions, expected 1), then green. The later missing-parent corruption
  fixture explicitly disables its own foreign-key enforcement before injecting the orphan.
* `cargo test --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-service --lib refresh_path_components_reuse_the_prepared_lookup_and_stay_contained`:
  red (0 executions on the cached statement, expected 60), then green.

Final commands used the exported lane target and four build/test threads:

* `cargo test --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-catalog --lib --tests`:
  267 passed, 0 failed, 0 ignored across 17 test binaries.
* `cargo test --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-catalog --doc`:
  pass (0 doctests).
* `cargo test --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-service`:
  332 passed, 0 failed, 1 existing ignored test (331 unit + 1 integration).
  The first run at default test concurrency had 330 unit passes, 1 failure and 1
  ignored: `sources_from_two_granted_roots_are_refused` failed at `scan.start`
  with `scan worker pool full`. It passed alone (1/1), then the complete suite
  passed with `RUST_TEST_THREADS=4`; no test or worker-pool code was changed.
* `cargo clippy --manifest-path C:/lw/wt/search-552/Cargo.toml -p loomward-catalog -p loomward-service --all-targets -- -D warnings`: pass.
* `cargo fmt --manifest-path C:/lw/wt/search-552/Cargo.toml --all --check`: pass.
* `git -C C:/lw/wt/search-552 diff --check`: pass.

The first default catalogue-suite invocation could not link the example because
Windows still had its running baseline executable open (LNK1104). Catalogue tests
were subsequently selected explicitly with `--lib --tests`, and doctests separately,
so the check did not need to relink a running probe.

**Changed:** catalogue pass/ancestry iteration, prepared refresh lookup, regression
tests, synthetic probe and evidence. No schema or public contract change.
**Verified:** the scoped commands and synthetic measurements above.
**NOT verified:** whole workspace (driver-owned), release-build or real-disk latency,
end-to-end watcher throughput, and independent PR review.
**Residual risk:** scoped searches still scan rejected candidates and can exhaust
their budgets; repeated SQL preparation is removed from refresh, but its per-component
lookups remain. The issue's writer/developer-clutter hypotheses remain unmeasured
and outside this lane. `HUMAN_TODO.md` was read and left unchanged; this patch needs
no new owner decision. Changes remain uncommitted for the driver.
