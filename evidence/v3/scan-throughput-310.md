# Product scan throughput — #310

11 October 2026, native Windows, non-elevated. **Shared host**, warm/uncontrolled cache;
hardware withheld. Aggregate receipts are appended in [scan-throughput.json](bench/scan-throughput.json).

The dominant cost was the catalogue writer's idle batch wait, not a per-file identity open.
The service sends one command and waits for its committed receipt. After applying that command,
the writer used `recv_timeout(1 ms)` to look for another command before COMMIT. This caller
cannot send its next command until COMMIT; the wait therefore expires every time. On this host,
the measured waits averaged approximately 13–15 ms. A typical small directory needs a staged
chunk and a listing publication: two waits per directory explain the directory-density effect
behind #310's approximately 315 entries/s.

The fix is `try_recv()`: batch commands already queued, commit immediately when the queue is
empty. Queue/transaction byte caps, the time cap, savepoints, cancellation, reference durability,
publication fencing and COMMIT-before-receipt remain in place. Existing prequeued batching tests
pass. No identity, containment, elevation or metadata-only rule was relaxed.

## Measurement and source scope

- Product path: a real `loomward-serve`, `scan.start` over HTTP, `jobs.get` to completion, then
  `tree.slice` to compare the committed root aggregate with progress. Fresh process and state
  per run. No fixture HTTP adapter or aggregate-only sink substitutes for the product scan.
- Synthetic S tier: 100,000 files, 1,000 descendant directories, 12,800,000 logical bytes.
  The service's `scan_throughput_fixture` example creates and registers an empty lab root before
  filling it with deterministic files. Its registry digest is a fixture placeholder, not a claim
  of independently verified manifest identity. An independent disk walk checks counts/bytes
  before every timed run; committed tree totals match exactly afterwards.
- Synthetic A/B is pinned to the lane's starting main, `bf2328020f66e780506103a2481532251efce1c6`.
  Both binaries contain identical phase diagnostics; the behavioural difference is the queue probe.
- A later fetch found #313 merged. Real-B therefore uses a separately built, pinned main
  `e9ed2b5`, with those same diagnostics, versus that same base plus the queue fix. The original
  lane remains based on `bf23280`; these two base groups must not be pooled.
- **real-B (owner-authorised, agent-picked)**: one explicit program-files folder, one product
  scan per variant. Its path was appended to the primary checkout's ignored scan-roots log before
  each scan. Fresh ignored state outside the root. No independent real-folder walk, content reads,
  cache clearing, elevation or host-policy changes. Public evidence contains rounded byte aggregates
  and no root path, filename, native identity, username or hardware.

## A/B

Final synthetic runs are receipt indices 1–6 (zero based), interleaved main/branch three times.
Index 0 preserves the initial diagnostic scan; it is excluded from the final median.

| Pair | Main elapsed s | Branch elapsed s | Main entries/s | Branch entries/s |
|---|---:|---:|---:|---:|
| 1 | 32.947 | 2.393 | 3,065 | 42,200 |
| 2 | 32.293 | 5.171 | 3,128 | 19,531 |
| 3 | 33.373 | 4.260 | 3,026 | 23,706 |
| Median | 32.947 | 4.260 | 3,065 | 23,706 |

Median improvement **7.73×**; elapsed ranges 32.293–33.373 s / 2.393–5.171 s (the branch varies about 2x run to run on this shared host). All throughput figures were measured with `LOOMWARD_SCAN_PROFILE=1`, so they include the profiler's overhead in both variants. All six have
complete coverage, exact counts/bytes, zero failed/skipped entries, no limit and no relist.
Elapsed is engine-reported; receipts also retain HTTP-observed wall time, including polling.

| Real-B | Main | Branch |
|---|---:|---:|
| Files / descendant folders | 31,464 / 4,266 | 31,464 / 4,266 |
| Logical / allocated GiB, rounded | 5.1 / 5.2 | 5.1 / 5.2 |
| Elapsed s | 134.280 | 4.761 |
| Entries/s | 266 | 7,504 |
| Coverage / failed / skipped | complete / 0 / 0 | complete / 0 / 0 |
| Empty-queue wait s | 127.793286 | 0.000918 |

One real pair gives **28.2×**, not a stable performance bound. Idle waits consumed 95.2% of
its baseline wall time. Different directory density, cache state and concurrent work prevent
equating this pair with #310's historical real-A or claiming the entire historical 60× gap closed.

## Phases and calls

`LOOMWARD_SCAN_PROFILE=1` enables process-cumulative native timers and aggregate stderr records.
The catalogue extends its existing writer timings. No protocol field or service endpoint was added.
Nested and parallel phase times **overlap**, so their sum is not wall time. Native listing time
includes callbacks/backpressure; `enumeration_queries` times only the directory-query OS calls.
Open timers cover native enumerator metadata opens, not token handles or every service-startup open.
Identity observations include compatible-width identity/volume checks; attribute queries overlap them.
Link-count observation is a separate placement pass, absent from the product scan path; it is not
silently inferred from enumeration. The per-file identity/open optimisation was therefore declined.

Calls below are identical between variants. Synthetic times are medians of the three final runs;
real times are the single pair. Writer timings include startup/grant commands as well as the scan.

| Phase | S calls | S main / branch s | Real calls | Real main / branch s |
|---|---:|---:|---:|---:|
| Directory enumeration OS queries | 2,008 | 0.157050 / 0.123101 | 8,636 | 2.413491 / 0.223720 |
| Native metadata opens | 1,024 | 0.032231 / 0.021532 | 4,280 | 1.388498 / 0.119481 |
| Post-open identity observations | 1,003 | 0.012359 / 0.009933 | 4,269 | 0.053184 / 0.048185 |
| Attribute queries | 2,028 | 0.006777 / 0.006672 | 8,550 | 0.026203 / 0.020869 |
| Containment checks | 1,001 | 0.283290 / 0.270737 | 4,267 | 0.184524 / 0.218378 |
| Normalised path queries, nested in containment | 2,001 | 0.281381 / 0.268809 | 8,533 | 0.178634 / 0.214696 |
| Staging chunks | 1,004 | 0.138324 / 0.101643 | 4,293 | 0.265281 / 0.178338 |
| Listing publication | 1,001 | 0.944278 / 0.745977 | 4,267 | 1.227349 / 0.878527 |
| Queue probes | 2,005 | 28.380686 / 0.000494 | 8,560 | 127.793286 / 0.000918 |
| Transactions / COMMIT phase | 2,008 | 1.798427 / 1.731157 | 8,563 | 1.989847 / 2.274838 |
| Rollup | 1 | 0.002735 / 0.002767 | 1 | 0.020469 / 0.014685 |
| Relists / additional repair passes | 0 | absent | 0 | absent |

The pipeline also records listing calls, entry attempts, enumeration worker/wall time,
writer staging/persistence waits and bottom-up finalisation time. The catalogue records
invalidation, multilink rebuilding and checkpoint time. All are retained in the JSON receipt.
Only 1,024 opens for 100,000 synthetic files and 4,280 for 31,464 real files rule out a per-file
enumerator open on these traversals; the native source still opens directories to verify identity.

The existing native S-tier scan bench also passed its exact manifest once: 100,000 files in
0.3757511 s, about 266,134 files/s, aggregate-only in-memory sink. It is a different synthetic
tree and sink, so it is a control, not an A/B throughput denominator.

## Tests, mutation and gates

`synchronous_listing_receipts_do_not_wait_for_future_commands` performs 64 synchronous
publications. A separate SQLite reader must see each new value when its receipt returns.
Its accumulated queue-probe budget is 25 ms, excluding SQLite and disk scheduling.
Initial old code failed with 0.830737 s of idle waiting. After the fix passed, restoring
`recv_timeout(1 ms)` failed again at runtime with 0.858907 s (exit 101); restoring `try_recv`
passed (exit 0). This is a behavioural red/green and revert/restore mutation, not a compile failure.
The benchmark's open-count assertion remains below file count on every accepted scan.

| Command | Lane base bf23280 + change | Refreshed base e9ed2b5 + change |
|---|---|---|
| `cargo fmt --all --check` | exit 0 | exit 0 |
| `cargo test --workspace` | 706 passed, 0 failed, 0 ignored; exit 0 | 713 passed, 0 failed, 0 ignored; exit 0 |
| `cargo test -p loomward-windows --all-features` | 92 passed; exit 0 | 92 passed; exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 | exit 0 |
| `git diff --check` | exit 0 | exit 0 |

All scan, containment, identity, errata and watch tests in these gates remain green and unchanged.
The new writer test is portable and does not open a native root. Existing native tests retain
their elevated-host refusal assertions. No new ignored test or weakened assertion.

Early harness attempts rejected completed scans when `roots.list` returned null totals and when
the requested slice size was below the protocol's minimum. These are not accepted benchmark
samples; the final six runs use a valid, bounded tree slice and committed-total assertions.
The ignored attempt logs remain available. No prior accepted receipt was overwritten.

## NOT verified and residual risk

Hosted elevated CI, cold cache, physical ReFS/fallback providers, cloud recall canaries, hung
volumes and real-A were not exercised here. A single real pair cannot qualify all program trees
or antivirus-filter behavior. Per-directory SQLite publication/commit work remains, and future
asynchronous producers may form smaller batches because the writer no longer waits for arrivals.
Commands already queued still batch; this lane does not implement receipt pipelining (#148).
`roots.list` returning null totals after a large completed scan is outside this fix; the committed
aggregate was verified through `tree.slice` instead. No commit or push was performed.

## P4 catalogue bench after the fix (driver, shared host)

The writer change replaces a 1 ms timed receive with an immediate probe, so asynchronous producers
must still form batches. `cargo run -p loomward-catalog --release --example bench -- --rows 1000000`,
interleaved, one variant swapped in at a time (only that line differs):

| Variant | Run 1 | Run 2 |
|---|---:|---:|
| try_recv (this change) | 74,963 rows/s | 90,652 rows/s |
| recv_timeout 1 ms (main) | 90,093 rows/s | 87,373 rows/s |

Ranges overlap and the change has the best run: no evidence of a P4 regression. Batching still forms
(up to 30 commands and 71 transactions per 1M-row run; `batch_wait_seconds` 0.0016 s in total).
