# Ranked token index: publish baseline, bulk-aware swap, migration (#520)

The three prerequisites [#520](https://github.com/Chris0Jeky/Loomward/issues/520) sets before the F2 token index
(`search_token` / `search_dirty`, [F2 evidence](search-relevance-f2.md)) may be enabled for a real catalogue. This lane
prepares; it does not enable: `features.federated_search` and the index both stay off by default.

Everything is **synthetic names built in-process** by `search_bench` (no enumeration, no HTTP, no personal data) on the
KRASPYON laptop (Windows 11 Home, 18 threads, 16 GB). **The host was shared with several other agent lanes and was not
quiet**: during the last hour of this lane even a directory listing of `%TEMP%` took minutes. Every run is one process,
peak working set read by polling the OS counter every 500 ms.

## Size: 5M, not 10M

The peak working set of a first-scan publish grows about 0.35 GB per million rows (0.69 GB at 1M, 1.04 GB at 2M,
2.09 GB at 5M with the token index, 1.82 GB without). 10M would need about 4 GB, over this lane's 3 GB per-process
budget on a 16 GB host shared with other lanes, so **5M is the largest size measured**. The memory is not the token
index: 5M without it peaks at 1.82 GB. Most of it is the deferred file-index builds and the sorter with
`temp_store=MEMORY` (an observation, not measured separately here).

## 1. Publish baseline (EndRun, first scan), off against on

Three rounds, interleaved off / on-queue / on-bulk in each round. *off* is `--no-token-index`; *on-queue* is the
pre-#520 binary (built from this branch before the swap change), *on-bulk* the #520 swap. Seconds:

| rows | config | end_run runs | median | peak WS |
|---|---|---|---|---|
| 2M | off | 55.0, 56.2, 66.1 | 56.2 | 0.93 GB |
| 2M | on, queue | 71.4, 90.7, 75.0 | 75.0 | 1.04 GB |
| 2M | on, bulk | 89.7, 121.6, 71.0 | 89.7 | 1.04 GB |
| 5M | off | 193.3, 330.8, 275.9 | 275.9 | 1.82 GB |
| 5M | on, queue | 237.2, 268.1, 255.9 | 255.9 | 2.09 GB |
| 5M | on, bulk | 286.8, 243.4, 221.7 | 243.4 | 2.09 GB |

The spread within one configuration (up to 71 percent at 5M, where *off* was slower than both *on* medians) is larger
than the difference between configurations, so these medians cannot rank the configurations. What they do bound: the
token index did not make the 2M or 5M first-scan publish worse than linear on this host (2M to 5M is 3.4x for
*on, queue*, 4.9x for *off*), unlike the single 10M run in the F2 evidence (1732 s against 68 s at 2M), which remains
unexplained and unrepeated. The catalogue grows 47 MB per million entries with the index (537 to 631 MB at 2M,
1352 to 1585 MB at 5M, measured before the final WAL checkpoint).

The token index's own share was measured inside the swap with a temporary timing build (not committed), one run each,
same binary, `LW_QUEUE` switching the path:

| rows | path | file rows + file indexes | token indexing | EndRun |
|---|---|---|---|---|
| 1M | queue (triggers, then `drain_tokens`) | 10.0, 10.3 | 5.4, 5.3 | 23.4, 23.6 |
| 1M | bulk (one pass) | 16.8, 8.9 | 10.0, 5.4 | 44.4, 22.6 |
| 5M | queue | 118.1 | 49.0 | 332.3 |
| 5M | bulk | 116.2 | 45.0 | 303.5 |

So the token index costs about 5 s per million entries of the first-scan publish (15 to 20 percent of `end_run` at 1M
and 5M), and nearly all of that is the FTS5 insert itself (analysis plus the `prefix='1 2 3'` postings), not the queue.

## 2. Bulk-aware swap

`swap_bulk` now drops the two token insert triggers inside the swap transaction (keeping their exact definitions from
`sqlite_schema`), copies the build, indexes the new keys in one pass from `bulk_dir` / `bulk_file`, and recreates the
triggers before the savepoint commits. A crash or a failed command rolls the whole thing back, triggers included.

- **Equivalence.** `bulk_first_scan_matches_the_incremental_path_row_for_row_and_query_for_query` now also runs with
  the index on: the incremental path (every key queued and drained) and the bulk swap must leave identical rows,
  identical schema (so the triggers come back byte for byte), an empty queue and identical token index content,
  compared as `fts5vocab` (term, key) pairs, with no neighbour, a live neighbour and a concurrent bulk neighbour, for a
  completed and a cancelled run. Mutation check: skipping the folder pass fails it.
- **Gain.** Small: at 5M the token work went from 49.0 s to 45.0 s and the trigger-free copy saved about 2 s
  (one run each); at 1M it is within noise. The issue's premise, that the queue and the read-back joins are the
  expensive part, is not borne out: the FTS5 insert is. A further saving would have to change the index itself
  (fewer prefix lengths, or no index on the first scan with a background build), which is a relevance decision, not
  this lane's.

## 3. Migration (opening an existing store with `ranked_index: true`)

Before #520, `search_index` queued every row and drained the whole queue inside the open transaction. Measured with
`search_bench --migrate` (build without the index, close, reopen with it):

| rows | before: open (all indexing inside) | WAL after open | after: open | backfill done after open | WAL after backfill |
|---|---|---|---|---|---|
| 1M | 10.1, 11.1, 9.3 (median 10.1 s) | 46.6 MB | 2.5, 2.6, 2.9 (median 2.6 s) | 6.8, 6.4, 6.4 s | 68.9 MB |
| 5M | 56.3 s (one run) | 241.6 MB | 21.3 s (one run) | 80.3 s | 71.5 MB |

10 s per million entries holding the open transaction (and the service's start) is too long: about 100 s at 10M. So
the backfill moved out of it:

- The open creates the index and its triggers and copies every existing key into `search_backfill`; that is all it
  does. A plain reopen of the same 1M store without the option took 4.4 s against 4.9 s with it (one run, busy
  host), so the queueing costs about half a second per million; the rest of the open is not the index.
- The writer indexes `search_backfill` in rounds of 20,000 keys, one transaction each, only while no command is
  waiting (a command sent mid-backfill waited 0.07 to 0.17 s), and drops the table when it is empty. A failed round
  rolls back and is retried a second later. It survives a close or crash: the table persists and the next open's
  writer resumes, whatever the option says then.
- A command that changes a waiting row indexes it at once (as a new key) and takes it out of the backfill, so nothing is
  indexed twice.
- `search_ranked` answers the typed `Error::RankedIndexBuilding` until the backfill is done, never a partial token
  tier. (Keys queued by a raw writer outside the writer still give `index_state: "building"` with results, as before.)
- WAL: many small transactions instead of one, so the log no longer grows with the catalogue (71.5 MB at 5M against
  241.6 MB); its high-water mark here is set by the bench's polling readers holding snapshots.
- Memory: bounded by construction (one round of 20,000 names, the FTS5 hash of 64 MB, the writer's 256 MB page cache);
  the migrate runs' process peak (0.63 GB at 1M, 1.82 GB at 5M) is the build phase before it, not the backfill.

Test: `an_opted_in_store_is_backfilled_in_the_background_to_the_queue_path_index` (no writer: `RankedIndexBuilding`;
a rename and a removal of waiting rows; reopen without the option; the writer finishes; the index equals the
queue-path index of the same tree, compared after `optimize`).

**Opt-in only.** The backfill runs only for a store opened with `ranked_index: true` or one that already has the
index. The service now opens the catalogue with `ranked_index: FEDERATED_SEARCH`, the same constant `session.hello`
reports as `features.federated_search`, which stays `false`; a service test asserts the store has no ranked-index object
while it is false.

## Crash safety

The catalogue crash sweep (default mode, `--features test-hooks`) runs with the index on and covers a bulk first scan,
so the swap's trigger drop and recreation are inside the swept boundaries; its recovery check requires an empty queue
and a token index equal to the rows' keys. The open-time backfill is not in the sweep (its stores start empty); its
rounds are ordinary transactions and its resumption is pinned by the test above.

## Not done

- No 10M run (memory budget, above), and no quiet-host run: the publish medians cannot rank the configurations.
- The retry of a failed backfill round has no test that forces the failure.
- The 1732 s 10M publish in the F2 evidence is still unexplained.

## Reproduce

```powershell
cargo build --release -p loomward-catalog --example search_bench
target/release/examples/search_bench.exe --rows 2000000 --publish-only [--no-token-index]
target/release/examples/search_bench.exe --rows 1000000 --migrate
```
