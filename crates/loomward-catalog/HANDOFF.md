# L2 catalogue receipt — 2026-10-09

Changed: uncommitted work in `C:/Users/jekyt/wt/lw-l2`, branch `feat/l2-catalog`, base and HEAD
`f4c6f1ae28dd999f6654f9cb18dffdeedf970bf1` (`arch/v03-architecture`). The driver commits.
Initial worktree was clean. No push, PR, merge, issue, real-disk scan or external application
action occurred. Network use was Cargo package-registry resolution only.

Files changed: root `Cargo.toml` has only the authorized glob member edit; `Cargo.lock` was
regenerated. New owned crate files: `Cargo.toml`, `src/{lib,db,model,writer,query,slice}.rs`,
`src/{catalog,state}.sql`, `tests/{open,writer,queries}.rs`, `examples/bench.rs`, `README.md`,
this receipt, and `evidence/catalog-1m-before-optimization.json`. Required benchmark exception:
`evidence/v3/bench/catalog-1m.json`. Coordinator-owned files were not edited.

## Acceptance

- MET: doc 41 section 6 schema is migration 1 for both STRICT databases; WAL/NORMAL/FKs and application/version markers are set.
- MET: foreign/newer/unowned-nonempty databases and UNC/device paths refuse; quick_check runs at open; precious migration backup, rollback and interrupted-backup retry are tested.
- MET: single writer thread, BeginRun/DirListing/DirFinal/EndRun, bounded queue and coalesced whole-listing commits; see the explicit oversized-listing exception in README.
- MET: refresh preserves IDs, identity renames and rename swaps; already-present directory identities reparent across listings with descendant depths preserved and cycles refused; unchanged listing revision stays stable; removed subtrees cascade.
- MET: logical, known allocated and allocation-unknown sums remain separate; sparse allocation and hard-link flags are tested; no reclaimability/effect claim.
- MET: slice folding, pre-order/depth/node bounds, Atlas/volume/root anchors, approximate_files/approximate_live and explicit arena overlay.
- MET: keyset children cursors bind listing_rev, anchor, sort and basis; aggregate changes also invalidate them.
- MET: path, inspect, bounded-budget search, generation-bound search cursors, and breakdown; contract projections validated against the actual JSON Schema.
- MET: random-tree SQL rollups equal independent brute-force sums (eight deterministic seeds, twenty directories each).
- MET: an abrupt child-process exit inside an uncommitted SQLite transaction preserves the prior committed listing; restart also recomputes incomplete totals from committed rows.
- MET: required 1M benchmark generated metadata in-process, matched the oracle and deleted the temporary directory.
- MISSED PERFORMANCE HYPOTHESIS: P4 is 38,925 rows/s against 250,000; doc 41 section 19's review trigger fires. No indexes were dropped or checks weakened.

## Verified

Windows, Rust/cargo 1.97.1. These exact commands completed with exit 0 on the final code:

```text
cargo test -p loomward-catalog
  open:    test result: ok. 5 passed; 0 failed; 0 ignored
  queries: test result: ok. 7 passed; 0 failed; 0 ignored
  writer:  test result: ok. 7 passed; 0 failed; 0 ignored
  (19 tests includes the crash-child helper; the crash test also observes exit code 17.)
cargo clippy -p loomward-catalog --all-targets -- -D warnings
  Finished `dev` profile ... (exit 0)
cargo fmt --all --check
  no output, exit 0
git diff --check
  exit 0; Git emitted only its LF-to-CRLF advisory
cargo test --workspace
  all above plus the existing 19 core tests passed
cargo clippy --workspace --all-targets -- -D warnings
  Finished `dev` profile ... (exit 0)
cargo run -p loomward-catalog --release --example bench -- --rows 1000000
  oracle.matched: true; temporary_directory_removed: true; exit 0
```

Red evidence observed before implementation/fixes: fresh/unsafe-open tests; writer identity,
accounting and recovery tests; query/fold/budget tests; aggregate-finalisation cursor test;
breakdown-cache test; interrupted-backup restart test; restart committed-total assertion; cross-parent directory identity move/cycle test.
An initial compile/API mismatch was corrected before treating a test run as red evidence.

| Receipt | Final measurement | Scope |
|---|---:|---|
| P4 | 25.716 s, 38,925 rows/s | Generation + writer backpressure, all indexes; target missed |
| P6 | p50 21.958 ms / p95 22.837 ms; 2,500 nodes; 1,383,727 bytes | Warm catalogue + serialization, 1M files |
| P7 | p50 0.795 ms / p95 0.850 ms | 200-row logical-size page in a 1,000-file directory |
| P8 | p50 32.458 ms / p95 34.775 ms; 21/21 budget hits | Absent substring, 2M SQLite-op budget |
| P14 | 126,676,992 bytes; 126.550 bytes/entry | Checkpointed catalogue, 1M files + 1,001 dirs |

## NOT verified

No 10M run, cold-cache run, HTTP round trip, live scanner/arena integration, renderer, Tauri,
process-memory calibration, Windows/native file identity acquisition, mapped-network-drive
test, personal catalogue or full Python/UI gate. There is no remote PR/CI result for this lane.
L1/L8 protocol crates are absent from this base, so projections were validated directly against
the checked-in contract rather than compiled against their future DTOs.

## Deviations, coordinator work and residual risk

All dependencies are ADR-V3-02-approved. `jsonschema` has default features disabled so its
validator cannot fetch external schemas. `time` sets this crate's declared minimum Rust to
1.88; the root manifest was not widened. Reader pooling, dataset instance locking, and
root/depth-1 breakdown warming are engine responsibilities; README names the calls. Metadata
root-registration is internal, never a path-accepting view command or filesystem grant.
Native cursors/references require L8 signing/remapping before transport. One oversized listing
can exceed the normal 200k-entry memory estimate while exclusively occupying the entry quota.
P4 remains materially below target; use the measured miss for the ADR/index strategy review.

Driver integration: coalesce the identical workspace glob edits from #108/#109, regenerate
the lockfile with all lanes, wire L7's writer receipts and live overlay, cap readers at four,
and let L8 map each contract-shaped projection and sign its internal cursors. Proposed status
updates (not made here): mark this catalogue slice implemented with Windows-only synthetic
proof; record the P4 miss and 10M/HTTP limits in docs/16 and handoff/CHECKPOINT; reconcile
LW-005/LW-009/LW-011's native catalogue portion without claiming the other UI/OS gates.

HUMAN_TODO.md was read and preserved: q-1 through q-4 remain open in this base. L2 requires no
owner decision to finish its synthetic proof; it does not resolve the real-disk grant item.

Recommended commits:

1. `feat(catalog): persist v3 observations with atomic refresh and bounded views`
2. `perf(catalog): record million-row Windows catalogue measurements`

Worktree is deliberately **not removal-ready**: all work is uncommitted. Final ignored survivor
is `target/` (rebuildable Cargo output); no personal data or database is retained. The driver
must commit/push before cleanup and run its integration/review gate. No deletion was attempted.
