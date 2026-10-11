# Async product scan staging — #148

11 October 2026, native Windows, non-elevated, **shared host**, warm/uncontrolled
cache. Hardware withheld. No commit or push. Receipts are retained in
[scan-async148.json](bench/scan-async148.json); the earlier #310 and P4f receipts
are unchanged.

## Changed

Measured against starting main `23440d101043b910c8aea9b64de06881dca25405`
(after #322 and #332). The frozen candidate differs in the service sink only;
its source SHA-256 is
`2f9b5d6466a046deb6847ccd90b8027219185d754cdb35fff3b52db5e170c01b`.
Both executable hashes are in every receipt. The aggregate `async148` record
pins run indices and corrects the truncated source labels of the real pair
without changing those original observations. All candidate runs used one
identical binary; all main runs used one identical binary.

`crates/loomward-service/src/sink.rs` reserves 4 MiB **before** converting each
engine chunk into catalogue observations, then uses `send_reserved` without
waiting for that chunk's receipt. At most eight chunk receipts per root remain
outstanding, including completed receipts not yet consumed. On the ninth send,
the oldest receipt is consumed first. The catalogue's existing shared quota
still bounds reservations across roots: 64 MiB total, including its 32 MiB
writer scratch reservation. No engine budget, queue, arena or scan cap changes.
The long-name/raw-UTF-16 test exercises the engine's 256-entry chunk shape within
the held reservation; `send_reserved` still validates and rejects insufficient
reservations.

`ListingDone` is enqueued after its chunks in the **same writer FIFO**, allowing
staging and publication to share COMMIT. Its engine acknowledgement waits for
all prior chunk receipts **and** its own publication receipt. The catalogue's
existing failed-chunk fence makes that listing upsert-only/partial if a chunk
failed; the sink reports the failure and cannot complete the run. This does not
require a separate chunk COMMIT before submitting the publication command.
Refreshes and `DirFinal` drain chunks before using their results; `EndRun` drains
before choosing its end state. Deferred failures remain sticky until the run is
ended as failed and the root is released. A deferred stale error is fatal rather
than incorrectly requesting a relist of the unrelated ticket that drained it.
Immediate input/revision refusals retain their existing mapping.

Receipt waits release the sink's state mutex. Revocation still commits through
the catalogue first, cancels the engine, then fences the sink; the sink cancels
its reservation/send token and drops retained receipts. Committed staging from
a revoked run remains unpublished under the existing catalogue policy. No later
sink call is accepted for the revoked grant. COMMIT-before-receipt, one
publication per listing, revision fences, reference durability, cancellation's
incomplete outcome, identity/containment/elevation guards and metadata-only
scanning remain.

The benchmark roots log now records the supplied source tag alongside its date
and root, so the authorised real pair records `async148` before each scan. No
new CLI option, endpoint, schema, dependency or writer policy was added.

## Verified

### Baseline phase diagnosis

Receipt index 0 is the initial main diagnostic, excluded from the final A/B.
The 100k synthetic service tier completed in **1.980 s** (51,001 entries/s),
with exact progress and committed tree totals. Pipeline persistence was
1.802 s; writer COMMIT 0.915 s, publication 0.523 s, staging 0.073 s,
state preparation 0.030 s and checkpoint 0.052 s. Native enumeration queries
took 0.095 s, metadata opens 0.014 s and containment 0.032 s. There were
2,008 transactions for 2,008 commands. Nested/parallel timings overlap;
they must not be added as wall time. The writer, especially COMMIT, remains
the dominant measured seam after the idle wait was removed.

Index 1 retains the pilot candidate (2.430 s), also excluded from the final
A/B. It already halved transaction count but was slower than the diagnostic;
no isolated pilot comparison is claimed as a gain.

### Synthetic service A/B

Receipt indices 2–7 are **A1/B1/A2/B2/A3/B3**, three runs each. Each uses a
fresh service process and catalogue, the same registered synthetic root/seed,
`scan.start` over HTTP, job completion and a committed `tree.slice` oracle.
An independent disk walk checks 100,000 files, 1,000 descendant directories
and 12,800,000 logical bytes before each timed run. All six have complete
coverage, exact totals, zero failed/skipped entries, no limit and no relist.
Own benchmarks did not overlap own builds or tests; other lanes shared the host.
Profiling was enabled in both variants.

| Pair | Main scan s | Candidate scan s |
| --- | ---: | ---: |
| 1 | 2.368 | 2.232 |
| 2 | 2.344 | 2.087 |
| 3 | 2.196 | 1.967 |
| Median [min–max] | 2.344 [2.196–2.368] | 2.087 [1.967–2.232] |
| Entries/s median [min–max] | 43,089 [42,641–45,976] | 48,386 [45,240–51,343] |
| HTTP wall s median [min–max] | 2.397 [2.296–2.407] | 2.135 [2.045–2.247] |

Every pair favoured the candidate. The scan median is **11.0% shorter**, or
**1.123× throughput**. The ranges overlap; this is a shared-host observation,
not a controlled-host or universal guarantee. It does not establish 250k/s.

| Phase s, independent median [min–max] | Main | Candidate |
| --- | ---: | ---: |
| Pipeline staging/publication waits | 2.145 [1.872–2.152] | 1.928 [1.772–1.983] |
| Pipeline aggregation/end | 0.192 [0.176–0.208] | 0.168 [0.136–0.225] |
| Native enumeration queries | 0.083 [0.047–0.091] | 0.089 [0.044–0.121] |
| Writer staging | 0.076 [0.073–0.079] | 0.076 [0.072–0.077] |
| Writer publication | 0.541 [0.525–0.567] | 0.520 [0.519–0.535] |
| Writer state preparation | 0.031 [0.029–0.048] | 0.004 [0.003–0.077] |
| Writer COMMIT | 1.193 [0.964–1.224] | 1.111 [0.933–1.185] |
| Checkpoint | 0.073 [0.056–0.078] | 0.038 [0.032–0.079] |
| Transactions (count in every run) | 2,008 | 1,007 |
| Maximum commands/transaction (every run) | 1 | 2 |

### Real-C: one authorised pair, negative end-to-end evidence

Because the lab A/B favoured the candidate in every pair, indices 8–9 record
one main/candidate pair on an agent-picked non-profile program folder. Both
date/path/`async148` entries were verified in the owner's ignored roots log.
State was fresh and outside the root. No additional real-root traversal,
content read, cache clearing or elevation. Public receipts contain aggregates
and rounded bytes only; no real names, paths, native identities or hardware.

| Real-C aggregate | Main | Candidate |
| --- | ---: | ---: |
| Files / descendant directories | 6,356 / 730 | 6,356 / 730 |
| Scan elapsed s | 0.574 | 6.034 |
| HTTP wall s | 0.674 | 6.107 |
| Pipeline staging/publication s | 0.520 | 0.419 |
| Pipeline aggregation/end s | 0.053 | 5.614 |
| Writer COMMIT s | 0.205 | 0.158 |
| Writer checkpoint s | 0.009 | 5.556 |
| Transactions | 1,466 | 734 |
| Coverage / failed / skipped | complete / 0 / 0 | complete / 0 / 0 |

Committed root totals match progress in both runs. The candidate improves the
staging seam but **regresses end-to-end time** in this pair. The measured delay
is in the existing SQLite checkpoint, outside staging. Its cause is unproven;
it is not dismissed as flaky or attributed to another lane. No repeat real
pair or checkpoint change was made. This pair does not qualify real-folder
speedup or non-regression.

### Regression and mutation proof

Nine new SQLite-backed sink tests cover asynchronous return while a separate
connection prevents COMMIT; eight outstanding chunks and 32 MiB reservation
backpressure; completed-receipt accumulation; single committed publication
after all chunks; failure in the middle of a queued listing; sticky failure
through `DirFinal` and `EndRun`; cancellation with queued chunks; durable
revocation with queued chunks; and conversion of a maximum-entry long/raw-name
engine chunk. Existing scan, sink, writer, watch and errata checks remain green.

The first async-return test failed on the original synchronous sink at its
runtime assertion, then passed with async staging. A synchronous-staging
mutation makes eight new tests fail at runtime. Removing/increasing the receipt
bound makes the remaining completed-receipt test fail. Thus every new test has
a demonstrated behavioural failure; no compile failure counts as mutation proof.

All **12 mutations were killed** (exit 101, executed failing tests): restore
synchronous staging; remove/increase the receipt bound; shrink the reservation
to 1 KiB; ignore a queued error; forget a queued error; omit the final drain;
omit the end drain; acknowledge publication before COMMIT; omit revocation
token cancellation; retain revoked receipts; treat cancellation as complete.
Source bytes were restored after each mutation, followed by the full gates.
Task-owned raw logs, mutation runner and frozen executables remain ignored
under `.loomward/async148/`.

Early test-helper attempts held the service DB mutex during publication and
deadlocked two tests; only this lane's test processes were stopped. Independent
SQLite blocker connections corrected the helpers. Another initial assertion
incorrectly expected revocation to delete durable staging; the final assertion
pins the existing unpublished/unchanged staging policy. These attempts were not
accepted gate passes or discarded benchmark observations.

| Required gate | Final output |
| --- | --- |
| `cargo fmt --all --check` | exit 0 |
| `cargo test --workspace` | exit 0; 788 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-windows --all-features` | exit 0; 93 passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| `py -3 scripts/verify.py` | exit 0; 227 Python tests, 1 existing symlink-privilege skip; 2 JS syntax, 9 JS boundary and 80 parity fixtures; Rust gates pass |
| `py -3 scripts/test_app.py --live-serve` | exit 0; ALL PASS (live serve), including personal-session metadata scan through the sink, exact fixture totals and unchanged files |
| `git diff --check` | exit 0 |

`npm.cmd --prefix app run build` also passed as the live-app prerequisite.
The restored service has 135 passing tests (including the nine new tests).

Recommended driver commits:

- `perf(service): stage scan chunks with bounded async receipts`
- `bench(service): record async scan staging comparisons`

Declined: checkpoint optimisation or extra real repeats, because the authorised
lane is async staging and one real pair. Whole-generation loading, schema/index
changes and wider receipt pipelines were not needed for this bounded seam.

## NOT verified

Hosted/Linux/elevated CI, controlled or cold cache, universal real-folder
performance, the real checkpoint delay's cause, physical power loss or hung
volumes, 10M performance and the 250k/s P4 target. No commit, push or publication.

## Residual risk

The synthetic gain is modest and ranges overlap on a shared host. The one real
pair is materially slower at end-of-run checkpoint despite faster staging;
retain that negative evidence when deciding whether to land the change. The
fixed reservation is sized for the current engine chunk shape; a future larger
chunk must remain within validation/quota or have this bound requalified.
