# #552 release-build query qualification (wave 12)

KRASPYON, Windows 11 Home 10.0.26100, rustc/cargo 1.97.1, 2026-10-19.
Starting HEAD: `b0927794a1836e82160356e726eaa3f023a684d3`.
Worktree `C:/lw/wt/w12-552`, branch `perf/552-search-n-plus-one`.
Synthetic SQLite metadata only; no disk scan or soak interaction.

## Reconciliation and scope

Read #552, including its writer hypotheses comment. The starting HEAD already has
the streaming-pass and recursive-ancestry fixes from #635, plus the earlier
negative-ancestry and containment corrections. The prior #552 receipts were debug
builds and lacked rare-query timings, complete page-byte replay and page SQL counts.
This change adds that missing qualification, without another production search
rewrite. Writer files, ranked-index and federated-search defaults are unchanged.

## Measurement

The existing `search_n1` probe builds 1,000,000 common-name files, plus 100 files
under disjoint depth-20 branches and two registered synthetic roots. Each query
uses `kind=file`, `limit=100`, a 50,000,000 VM-operation budget and no wall-clock
budget. The narrow grant permits only the other root: its zero-hit budget cut is
expected. Queries are timed three times, including the first call; report medians,
not p95. Database creation and opening/integrity checking are outside the timer.

Before: temporarily substitute **only** `crates/loomward-catalog/src/search.rs`
from `4babf67` (the pre-#635 implementation), keeping today's other code and the
same probe/fixture. After: restore that file byte-for-byte from the starting HEAD.
This compares search algorithms without changing the catalogue schema or writer.

SQL counts come from SQLite's statement-execution trace in a **separate untimed
call**, so tracing thousands of FTS-internal statements does not inflate latency.
`application_statements` excludes trace SQL starting `--` (FTS internal statements);
it includes reader repair checks, transaction boundaries and search metadata reads.
`sql_statements` includes both. FTS and ancestry columns count application queries.
The trace hook and its dependency feature exist only with `test-hooks`.

| Query / first page | Before ms | After ms | Application SQL before → after | All SQL before → after | FTS before → after | Ancestry before → after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Common `common` | 3.078 | 3.409 | 9 → 9 | 562 → 562 | 2 → 2 | 1 → 1 |
| Rare `common-999999.txt` | 15.576 | 17.790 | 11 → 11 | 3,112 → 3,112 | 4 → 4 | 1 → 1 |
| Narrow grant, `common`, budget cut | 1,845.524 | 1,093.337 | 2,553 → 8 | 827,686 → 561 | 2,547 → 2 | 0 → 0 |
| `deep`, disjoint depth-20 branches | 8.852 | 9.114 | 2,011 → 110 | 3,136 → 1,235 | 4 → 4 | 2,001 → 100 |

The largest measured latency remains the narrow-grant common query. The existing
streaming fix removes thousands of MATCH restarts. Budget-cut watermarks differ
because the optimized version can examine more candidates within the same VM
budget; this is not a fixed-work throughput comparison. Other latency differences
are inconclusive on a shared host; the ancestry statement reduction does not
establish a wall-clock speedup here. No additional runtime change is justified by
this sample. The after invocation's separate paired SQL resolver probe took
426.620 ms uncached / 215.406 ms prepared for 1,024 depth-20 paths; both execute
21,504 statements. This is the resolver's exact SQL, not end-to-end service or
watcher latency; the service already reuses its prepared child lookup.

Raw receipts: [before](bench/search-n1-552-release-w12-before-1m.json),
[after](bench/search-n1-552-release-w12-after-1m.json).

```powershell
$env:CARGO_BUILD_JOBS = '4'
$env:CARGO_TARGET_DIR = 'C:/lw/target/lane-w12-552'
cargo run --manifest-path C:/lw/wt/w12-552/Cargo.toml --release -p loomward-catalog --features test-hooks --example search_n1 -- C:/lw/wt/w12-552/.loomward/search-n1-release-1m 1000000
```

The same command is run once per source variant, reusing its fixture. The final
working tree has no `search.rs` diff. After restoring a historical comparison,
write the restored source with a fresh timestamp before rebuilding: copying its
old timestamp can make Cargo reuse the historical binary.

## Regression proof

`search_pages_and_cursors_match_the_pre_streaming_reference_bytes` uses a stable
synthetic catalogue instance, 512 rejected candidates, depth-20 ancestry, hidden
ancestors, Unicode names, exact/prefix/token/substring tiers and extension/size
pagination. It compares every hit field, ordering, cursor and page metadata byte
against the checked-in fixture captured from `4babf67`; all pages run to exhaustion
without a budget cut. It does not assert byte equivalence of budget-limited cuts.
With `test-hooks` it also pins FTS and ancestry execution counts for selected pages:
`[(5, 2), (4, 1)]` (FTS, ancestry), versus the historical implementation's
`[(9, 23), (4, 21)]`. The common page resolves one visible chain and a hidden chain
it must exclude; the rare page resolves only the visible chain. The historical
run passed the byte comparison and failed the cost assertion; the final current
run passes both. The checked-in fixture contains seven pages across three cases.

Every Cargo command uses the lane target above; suite runs additionally set
`RUST_TEST_THREADS=3`. Commands use the worktree manifest, never the primary:

| Command | Result |
| --- | --- |
| `cargo test --manifest-path C:/lw/wt/w12-552/Cargo.toml --release -p loomward-catalog --features test-hooks --test search_n1` | 1 passed, 0 failed |
| `cargo test --manifest-path C:/lw/wt/w12-552/Cargo.toml -p loomward-catalog` | 303 passed, 0 failed, 1 existing ignored benchmark |
| `cargo test --manifest-path C:/lw/wt/w12-552/Cargo.toml -p loomward-service` | 400 passed, 0 failed, 1 existing ignored test |
| `cargo clippy --manifest-path C:/lw/wt/w12-552/Cargo.toml --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --manifest-path C:/lw/wt/w12-552/Cargo.toml --workspace --no-fail-fast` | 1,705 passed, 0 failed, 4 existing ignored tests |
| `cargo fmt --manifest-path C:/lw/wt/w12-552/Cargo.toml --all --check` | PASS |
| `git -C C:/lw/wt/w12-552 diff --check` | PASS |

Raw gate logs are under `C:/lw/target/lane-w12-552/` (`catalog-test.log`,
`service-test.log`, `workspace-clippy-all-features.log`, `workspace-test.log`,
`fmt-check.log`, `replay-current.log` and `replay-reference-count-red.log`).
The synthetic 1M fixture and temporary source-comparison files were removed
after saving both receipts and restoring the tracked source; target logs remain.
The workspace tests additionally leave ignored `target/test-roots/` and
`python/loomward/__pycache__/` in this worktree. The first is the service
`root_coverage` test's explicit fixture parent, not a Cargo build target; all Cargo
builds used the assigned lane target. Automatic approval review rejected cleanup
of these generated directories/bytecode with "blocked by policy", before the
command executed. They remain ignored and are recorded for the driver's handoff.
No unexpected gate failure needed a retry. The deliberate historical count
failure above is regression proof, not a failure of the retained implementation.

## Limits

No 10M release measurement, cold-cache experiment, quiet-host latency claim,
real-inventory scan, service/watcher latency, app/browser test or hosted CI.
Narrow-grant searches still inspect rejected candidates and can exhaust their
budget. The writer hypotheses belong to the other lane. `HUMAN_TODO.md` was read
and left unchanged; this work requires no owner decision. Changes are left
uncommitted for the driver; no push, PR or merge was performed.

**Changed:** release probe, feature-gated SQLite trace hook, byte-level replay and
statement-count regression, synthetic receipts and this evidence.

**Verified:** 1M release comparison, seven complete byte-identical fixture pages,
the cost regression and all local gates above at the recorded base plus this patch.

**NOT verified:** 10M release/cold-cache or real-disk latency, service/watcher
throughput, app/browser, hosted CI and independent PR review.

**Residual risk:** broad common queries under narrow grants still consume their
work budget. Shared-host timings do not establish a stable latency distribution.
Existing N+1 fixes were already present; no additional production optimization
was made. `HUMAN_TODO.md` is unchanged and no new owner action is required.
