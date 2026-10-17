# Find relevance and latency, ranking v1 (LW-113 F2)

Lane F2 of [docs/46](../../docs/46-find-and-ask.md) section 10: name-tuned token analysis, the short-query path,
ranking v1 (weighted reciprocal-rank fusion of the trigram, token and context lists, with exact, prefix, token-start
and recency boosts and a key tie-break), the relevance harness and the semantic-corpus generator. Receipts:
[bench/search-relevance-f2.json](bench/search-relevance-f2.json) (1M),
[-2m](bench/search-relevance-f2-2m.json) and [-10m](bench/search-relevance-f2-10m.json).
Everything here is **synthetic names built in-process** (no enumeration, no HTTP, no personal data), measured on the
KRASPYON laptop (Windows 11, 16 GB). Timings are warm and uncontrolled, taken while other agent lanes shared the machine
(an earlier pass of the same runs, during a saturated-disk period with queue length 15 to 30, was discarded: relevance was
identical and every latency was higher). Compare the columns of one table, not absolute numbers across hosts.

## The index is off by default

Nothing serves ranked search yet (`features.federated_search` is false), so the token index is opt-in:
`Catalog::open` builds no `search_token` or `search_dirty`, queues nothing and refuses `search_ranked` with
`Error::RankedIndexDisabled`; `Catalog::open_with(.., &CatalogOptions { ranked_index: true })` creates and backfills it
(a store that already has it keeps maintaining it whatever the option says). Everything in this file, the crash sweep and
the F2 tests ran with it on; a test pins both modes, including that a store opened without it gains no object. **Turning
it on for real catalogues is F3/F5's job, together with three things recorded in
[#520](https://github.com/Chris0Jeky/Loomward/issues/520): (a) a controlled 10M baseline without the token index,
(b) a bulk-aware swap that builds the token index directly instead of queueing, (c) a measured open-time migration at
1M and 10M.** The cost figures below are why.

## What was measured

`Reader::search` (v1 pass order, unchanged) against `Reader::search_ranked` on 1,001,303 entries
(and again on 2,002,303 and 10,010,303): the filler is invented-word files that share no word
with the planted part, plus 243 planted files in 60 folders (invoices by year,
thesis drafts, game mods, photo dumps with camera names, build outputs, versioned documents, hostile names) with
near-miss decoys. 76 labelled queries (52 known item, 14 attribute,
10 concept), each with graded relevant paths (2 is what the person wants, 1 a near miss). Seed
42; the ranker is fed the fixed clock 2026-10-17 so recency is deterministic. MRR@10 counts the first relevant path
of any grade, nDCG@10 uses gain 2^grade-1, recall@50 is the share of relevant paths found in the top 50.

**The corpus and the ranker were written together.** Ground truth is the intent of each planted scenario, not an
independent judgement, and the queries exercise exactly the seams the ranker was built for. Read the table as
"the ranker serves these scenarios and v1 does not", not as a general quality score. Personal roots are not
evaluated here (q-4, q-18).

## Relevance, 1,001,303 entries

| system | MRR@10 | nDCG@10 | recall@50 | queries cut by the 250 ms budget |
|---|---|---|---|---|
| search v1 (pass order), the baseline | 0.770 | 0.725 | 0.733 | 0 |
| ranked, trigram list only | 0.770 | 0.751 | 0.733 | 0 |
| ranked, token list only | 0.809 | 0.791 | 0.773 | 0 |
| ranked, context list only | 0.263 | 0.240 | 0.211 | 0 |
| ranked, trigram + token | 0.809 | 0.791 | 0.773 | 0 |
| ranked, fused (trigram + token + context) | 0.993 | 0.981 | 0.949 | 0 |

The same table at 2,002,303 and 10,010,303 entries is identical to three decimals
(the fused row cut 1 query by the 250 ms budget at 10M, none below); only the filler grows,
and it shares no word with the queries.

By query class:

| class | queries | v1 MRR / nDCG / recall@50 | fused MRR / nDCG / recall@50 |
|---|---|---|---|
| attribute | 14 | 0.93 / 0.77 / 0.86 | 1.00 / 1.00 / 1.00 |
| concept | 10 | 0.25 / 0.13 / 0.06 | 0.95 / 0.96 / 0.92 |
| known_item | 52 | 0.83 / 0.83 / 0.83 | 1.00 / 0.98 / 0.94 |

By scenario group:

| group | queries | v1 MRR / nDCG / recall@50 | fused MRR / nDCG / recall@50 |
|---|---|---|---|
| build | 6 | 0.00 / 0.00 / 0.00 | 1.00 / 0.83 / 0.50 |
| filler | 30 | 1.00 / 1.00 / 1.00 | 1.00 / 1.00 / 1.00 |
| hostile | 9 | 0.78 / 0.78 / 0.78 | 1.00 / 1.00 / 1.00 |
| invoices | 6 | 0.33 / 0.30 / 0.18 | 1.00 / 1.00 / 0.95 |
| mods | 3 | 0.67 / 0.67 / 0.67 | 1.00 / 1.00 / 1.00 |
| photos | 6 | 0.67 / 0.54 / 0.51 | 1.00 / 1.00 / 1.00 |
| thesis | 6 | 0.58 / 0.51 / 0.43 | 0.92 / 0.93 / 0.91 |
| versions | 10 | 1.00 / 0.80 / 1.00 | 1.00 / 1.00 / 1.00 |

docs/46 section 9 gates for the Find stage:

- *Fused beats the best single source on nDCG@10*: fused 0.981 against 0.791 for the best single list. Met.
- *recall@50 of 0.9 or better on known-item queries*: fused 0.942 (v1 0.827). Met.

Where fused still loses recall, by design or by a known limit:

- **Several words mean all of them.** `thesis draft` finds the drafts and not the chapter files that only say
  thesis (grade-1 near misses), and `<project> release exe` does not return the debug build's `<project>.exe`
  (grade 1) because `release` is not in its name or its two nearest ancestors.
- **No stemming.** `invoices 2023` reaches the 2023 folder's files through the `Invoices` folder, but not
  `invoice acme corp 2023-03.pdf` (grade 1): `invoices` is not a prefix of `invoice`.
- **Context is two ancestors**, not the whole path.

## Latency (first page, limit 50, 250 ms soft budget, 21 warm runs; p50 / p95 ms)

| query class (example text at 1M) | 1M v1 | 1M fused | 2M v1 | 2M fused | 10M v1 | 10M fused |
|---|---|---|---|---|---|---|
| camel_hump (`texture pack`) | 8.7 / 24.6 | 9.4 / 22.9 | 4.6 / 5.6 | 5.8 / 8.7 | 11.9 / 15.3 | 14.8 / 19.1 |
| concept_folder_context (`invoices 2023`) | 5.1 / 7.7 | 6.6 / 13.0 | 5.7 / 7.3 | 6.3 / 7.7 | 6.2 / 6.8 | 10.7 / 19.1 |
| digits (`333333`) | 3.9 / 5.2 | 4.9 / 8.1 | 7.8 / 10.8 | 8.7 / 9.4 | 40.9 / 46.3 | 49.4 / 59.8 |
| inner_substring (`recu`) | 2.1 / 3.8 | 1.9 / 3.3 | 2.3 / 3.8 | 4.0 / 5.1 | 4.6 / 6.8 | 4.2 / 5.6 |
| no_match (`zzqqxx`) | 2.0 / 4.5 | 2.2 / 3.3 | 2.0 / 2.6 | 2.3 / 3.0 | 1.6 / 2.2 | 1.8 / 2.3 |
| one_char (`k`) | 1.5 / 4.1 | 8.1 / 14.5 | 0.7 / 1.2 | 4.9 / 6.2 | 0.9 / 1.3 | 3.1 / 4.7 |
| prefix_4 (`brec`) | 1.1 / 1.5 | 10.4 / 15.7 | 1.8 / 2.4 | 20.6 / 23.9 | 4.0 / 6.9 | 73.7 / 92.5 |
| two_chars (`ka`) | 1.2 / 1.8 | 5.8 / 8.0 | 1.2 / 1.9 | 5.7 / 7.0 | 1.3 / 1.6 | 3.7 / 4.1 |
| two_words_known_item (`ruquayo tanquagan 29065`) | 48.5 / 76.8 | 61.9 / 77.6 | 44.1 / 60.1 | 57.3 / 72.4 | 66.3 / 87.4 | 135.7 / 181.6 |
| word (`brecunor`) | 4.7 / 8.3 | 6.6 / 7.3 | 3.6 / 4.9 | 8.0 / 10.7 | 6.7 / 9.3 | 13.0 / 16.6 |
| word_and_extension (`brecunor`) | 90.4 / 108.8 | 64.3 / 100.1 | 16.2 / 18.8 | 24.9 / 29.0 | 15.0 / 17.0 | 28.9 / 30.6 |

P8 (docs/46 section 9), *300 ms p95 or `budget_hit` for a substring query at 10M*: holds. The inner-substring page is
5.6 ms p95 at 10M fused (6.8 ms v1), and the slowest fused class at 10M is the three-token known item at
182 ms p95 (87 ms v1), all under 300 and none cut.
*Token and prefix queries 50 ms p95 at 1M*: holds for fused ranking on every token, prefix, short, digit and camel-hump
query (the slowest is camel_hump at 23 ms; the three-token known item is
78 ms, v1 77 ms, and the word with an extension filter 100 ms, v1 109 ms,
both dominated by the v1 trigram passes). At 10M the unindexed four-letter prefix grows to
92 ms and the digit query to 60 ms; there is no gate at that size. Ranking adds a few
milliseconds to a few tens over `search`. The runs are single passes on a shared machine: do not read a trend across
sizes from small differences.

## What the token index costs

Index bytes: trigram 92.8 MB at 1M (92.6 B/entry, unchanged by this lane), token
42.8 MB (42.8 B/entry, `prefix='1 2 3'`, `detail=none`); at 10M 95.6 and 43.6 B/entry.

Write path: `search_bench --rows N` (first-scan bulk build, then a watcher-style relist of 100 folders), with the
token triggers dropped (`--no-token-index`, so nothing is queued or drained) against the shipped schema, seconds:

| rows | run | token triggers | publish | end_run (swap + index) | relist 100 dirs | catalog bytes |
|---|---|---|---|---|---|---|
| 1M | busy host | off, off | 14.2, 12.9 | 35.1, 36.4 | 15.7, 24.1 | 367.1 MB |
| 1M | busy host | on, on | 24.7, 10.4 | 70.9, 62.2 | 38.6, 37.5 | 393.3 MB |
| 1M | quiet host | off, off | 5.3, 9.4 | 11.3, 18.1 | 5.6, 7.5 | 367.1 MB |
| 1M | quiet host | on, on | 7.8, 9.8 | 38.0, 28.3 | 15.8, 12.4 | 393.3 MB |
| 2M | quiet host | off | 12.6 | 36.6 | 10.1 | 740.5 MB |
| 2M | quiet host | on | 15.6 | 39.0 | 10.5 | 766.7 MB |

The spread is too large for one ratio: the paired `end_run` runs differ by +7 to +240 percent, because the machine was
shared and every run is one transaction of 1M to 2M rows. What can be said is that a second full-text index is a real
write-path cost (a second FTS5 insert per entry, plus a queue insert, a join read and a queue delete for the
drain), that it adds 26 MB per million entries to the catalogue, and that it needs a controlled measurement before a
claim stronger than that. At 10M the whole first-scan publish (both indexes, the file indexes and the aggregates) took
1844 s on this host, `end_run` 1732 s against 68 s at 2M: far worse than linear, and
there is no 10M run without the token index to say how much of that is this lane's. A bulk-aware path that skips the
queue for the swap is the obvious saving and is not done here.

The 10M receipt was taken before the final review round (Unicode token characters, revision-bound cursors, the option);
those change the index only for scripts with combining marks and the cursor only for paging, and the 1M and 2M receipts
were retaken after them with identical relevance.

## Limits of ranked mode (by design, recorded for the lane that serves it)

- **Not an enumeration.** Every page of a query is cut from one pool that depends only on the query, its filters, the
  ranking options and the page size (the cursor binds all four), so pages neither overlap nor skip while the
  catalogue is unchanged. The pool is at least 100 hits deep (twice the page size above 50) and at most 500 are
  answered. A budget cut ranks what was read and says `budget_hit` with no scan position to resume. `search` (v1)
  keeps its keyset cursor and its guarantees untouched.
- **Bounded candidate lists.** The trigram list is read no deeper than `search` would read for the same page (a later
  pass costs far more than an earlier one); the token list reads index matches in key order in windows of 256 until
  256 pass the request's filters or 2,048 were looked at, then keeps the shorter names (twice the page size, 100 to
  1,000); the context list reads up to 4,096 entries per kind from at most 2,048 folders and keeps the same depth. A
  very common token on a very large catalogue is ranked within that window.
- **Paging continuity.** The cursor binds the catalogue revision: after any publication it is refused with
  `StaleGeneration` (a live scan makes that frequent). Continuity is not promised across a `budget_hit` page, whose pool
  may be cut elsewhere on the next page.
- **Token characters.** The index tokenizer treats letters, marks, numbers and symbols as token characters (a superset
  of the analyser's), so one analysed token, including Thai, Devanagari and Arabic words with vowel signs, is one index token and
  a query term is never a phrase (which `detail=none` refuses). A query text the index still cannot parse skips the token
  and context lists instead of failing the call.
- **Camel humps and digits** are split in the index (`ThesisChapter3.docx` is `thesis chapter 3 docx`); diacritics are
  folded by the index but not by the Rust-side boosts, so a diacritic-only mismatch costs a boost, never a result.
- **Not wired to the service.** `features.federated_search` stays false and `search.query` still answers with v1;
  serving ranked results (with `score` and the match explanation) belongs to the lane that flips that flag.

## Reproduce

```powershell
cargo build --release -p loomward-lab --example search_relevance
target/release/examples/search_relevance.exe --filler 1000000 --seed 42 --out receipt.json   # 2000000, 10000000
cargo build --release -p loomward-catalog --example search_bench
target/release/examples/search_bench.exe --rows 1000000 [--no-token-index]
```

The 10M run needs about 4 GB of catalogue plus up to 3 GB of WAL and took about 31 minutes here.
