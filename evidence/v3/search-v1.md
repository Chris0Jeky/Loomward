# Search v1: FTS5 name index over granted roots (LW-113, q-34, q-36)

12 October 2026, native Windows 11 (DESKTOP-IHKOOJS), non-elevated, warm and uncontrolled cache.
**Shared host:** other agent lanes were building and testing during every run, so absolute times
carry noise; the A/B rows were interleaved run by run to spread it. Synthetic names only; no real
folder was scanned. Raw receipts: [`bench/search-v1.json`](bench/search-v1.json).

## What was built

- **SQLite capability.** The bundled `libsqlite3-sys` 0.36.0 (SQLite 3.51.1) is compiled with
  `-DSQLITE_ENABLE_FTS5` by its own `build.rs`; trigram (3.34+) and `contentless_delete` (3.43+)
  are available. No build flag or crate feature change was needed.
- **Index.** One contentless FTS5 table, `search_name`, trigram tokenizer, `case_sensitive 0`,
  `detail=full`, `contentless_delete=1`, over folder and file names (keys: folder id, file id + 2^48).
  The text is the name between two sentinels (U+0001 before, U+0002 after), so whole-name and
  name-start matches are phrase queries on the same index as substrings. Root folders (the grants)
  are not indexed. Path, size, kind and modified time come from the catalogue rows by key, so a
  folder rename never rewrites its descendants' entries. Labels, collections and content terms are
  meant to arrive as their own tables keyed the same way.
- **Maintenance: triggers, same transaction.** Six SQL triggers on `dir` and `file` keep the index
  exact in every writer transaction, and in any other connection that writes the tables. The
  first-scan bulk build (#148) is indexed as its rows join the live tables inside `EndRun`; a build
  is never searchable before it publishes. Forget (#382) deletes the rows (the triggers drop their
  entries) and then runs FTS5 `optimize`, which rewrites the index without the tombstoned terms; with
  `secure_delete` the freed pages are zeroed. A store without the index gains it, filled from its
  rows, on its next open.
- **Query.** Ordered passes, each one FTS5 expression whose `NOT` clauses exclude the earlier ones,
  so the tiers are disjoint and decided by SQLite under one case fold: exact (whole name, or name up
  to a dot), prefix, token (after one of 16 separators), substring; for several words, all words at
  word starts, then all words anywhere. Folders first, then files, in key order; the cursor is a
  keyset position bound to the filters and the catalogue instance, not `catalog_rev`. Filters:
  root, kind, extension, size range, modified range. An empty text lists by filters only (one
  extension largest first, via `file_by_ext`). Per hit: active grant, requested root, no
  `absent_pending` ancestor. Each page stops at 250 ms of work (service default) with
  `budget_hit` and a resumable cursor.
- **Contract.** `search.query` keeps its name and gains `max_bytes`, `modified_after`,
  `modified_before`; its result is now `SearchPage` (hits with row, root, root-relative path, match
  kind and UTF-16 highlights; `index_state`, `partial`, `substring_searched`, per-root coverage).
  Owner-local and grant-scoped; no MCP or Ask exposure (LW-067).

## Decisions by measurement

| Question | Measured | Decision |
|---|---|---|
| `detail=none` vs `full` (1M names, Python sqlite 3.50 probe) | trigram 34 vs 68 B/entry raw; `none` cannot run phrase queries | `full`: exact tiers and disjoint passes need phrases; `none` would need Rust re-verification under a case fold that can disagree with SQLite's |
| Separate unicode61 token table | +18 to 25 B/entry and about 1.3 s per million inserts (Python probe) | not built: word-start matching is a separator-anchored trigram phrase on the one index; only 1-character word starts are lost (reported as `substring_searched: false`) |
| FTS5 `hashsize` 1 MiB vs 64 MiB (1M inserts, one transaction) | 5.86 s vs 3.70 s | 64 MiB, set once at index creation |
| `contentless_delete` vs `'delete'` command (100 relist transactions) | 2.32 s vs 1.56 s | `contentless_delete`: a delete never needs the old text, so a mismatch cannot corrupt the index |
| FTS5 `secure-delete` option vs `optimize` for Forget | `secure-delete` left the forgotten terms on disk; `optimize` removed them | `optimize` after the purge |
| Triggers vs batch maintenance | see the write path below | triggers: every write path, including future ones and raw test connections, stays exact |

## Results (1,000,000 files, 1,001 directories)

Command: `cargo build --release -p loomward-catalog --example search_bench`, then
`search_bench --rows 1000000` (index) and `search_bench --rows 1000000 --no-index` (index and
triggers dropped after open), three interleaved runs each; P4 with the unchanged
`examples/bench.rs --rows 1000000`, branch binary against a main (`00c3d1f`) binary, three interleaved runs.

### Size

| | Bytes per entry |
|---|---:|
| Search index (`dbstat`, all `search_name` tables) | 100.9 |
| Catalogue without the index (`--no-index`) | 266.3 |
| Catalogue with the index | 366.7 |
| P4 bench catalogue, main → branch | 229.9 → 311.2 |

The index adds about 38 percent to the catalogue file (P14's 200 B/entry target was already
exceeded on main).

### Write path

| | Without index | With index |
|---|---:|---:|
| Bulk first scan: publish s (staging) | 4.65–5.07 | 3.89–4.99 |
| Bulk first scan: `EndRun` s (swap, indexes, index fill) | 5.52–5.63 | 8.15–10.40 |
| Through-`EndRun` rows/s | 94,159–98,491 | 66,185–83,147 |
| Watcher relist, 100 × 1,000-file folders, 10 renames + 10 removals each, s | 3.29–3.56 | 4.37–6.37 |
| P4 bench through-`EndRun` rows/s (main → branch) | 101,323–119,302 | 79,793–88,592 |

Index maintenance costs about 25 percent of first-scan throughput (paid once, inside `EndRun`)
and about 10–30 ms per relist transaction that changes names (FTS5 writes a segment per commit and
a contentless delete marks a tombstone). Both stay correct under the publication fence: the index
changes in the same transaction as the rows.

### Queries (first page, limit 50, warm, 21 runs after a warm-up, includes hit serialisation)

| Query | Hits | First tier | p50 ms | p95 ms |
|---|---:|---|---:|---:|
| `budget` (common word) | 50 | prefix | 1.9 | 2.1 |
| `udge` (inner substring) | 50 | substring | 2.2 | 2.5 |
| `4242` (digits) | 50 | token | 2.4 | 2.7 |
| `annual report` (two words) | 50 | prefix | 4.8–5.2 | 5.5–5.9 |
| `invoice` + extension `pdf` | 50 | prefix | 5.1–5.3 | 5.5–5.8 |
| `photo` + min size + modified after | 50 | prefix | 61.7–62.5 | 63.5–64.8 |
| `pl` (two characters) | 50 | prefix | 0.8–0.9 | 1.0–1.5 |
| no text, extension `mp4`, largest first | 50 | filter | 0.6 | 0.6–0.8 |
| `zzqqxx` (no match) | 0 | – | 1.8–1.9 | 2.1–2.3 |
| `report` + near-maximum size (1 in 30,000 match) | 34 | prefix | 250.5 | 250.6 (`budget_hit`) |

P8 (p95 300 ms or `budget_hit`) holds for every query. The last row is the worst case, a selective
filter on an unselective text: every candidate is examined, so the page stops at its 250 ms budget
with a cursor. Before that budget existed the same query took 500–650 ms.

## Not measured

- The E: scale lab trees (`mixed-1000000`, `mixed-10000000`) are enumeration fixtures; the
  service registers only lab roots it creates empty (`lab::register`), so they were not scanned
  into a catalogue. The 1M figures above are the in-process synthetic catalogue. 10M was not run
  (the bench caps at 2M rows; a 10M catalogue with the index is about 3.7 GB).
- No HTTP round trip, cold cache, quiet host or process-memory figure for search.
- Content terms, Everything integration, an elevated NTFS helper and a semantic layer are later lanes.
