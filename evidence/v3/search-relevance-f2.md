# Find relevance and latency, ranking v1 (LW-113 F2)

Lane F2 of [docs/46](../../docs/46-find-and-ask.md) section 10: name-tuned token analysis, the short-query path,
ranking v1 (weighted reciprocal-rank fusion of the trigram, token and context lists, with exact, prefix, token-start
and recency boosts and a key tie-break), the relevance harness and the semantic-corpus generator. Receipts:
[bench/search-relevance-f2.json](bench/search-relevance-f2.json) and [bench/search-relevance-f2-2m.json](bench/search-relevance-f2-2m.json).
Everything here is **synthetic names built in-process** (no enumeration, no HTTP, no personal data), measured on the
KRASPYON laptop (Windows 11, 16 GB) while about ten other agent lanes shared its CPU and a saturated disk (queue
length 15 to 30). Timings are warm and uncontrolled; compare the two columns of one table, not absolute numbers
across hosts.

## What was measured

`Reader::search` (v1 pass order, unchanged) against `Reader::search_ranked` on 1,001,303 entries:
1,000,000 invented-word filler files that share no word with the planted part, plus
243 planted files in 60 folders (invoices by year, thesis drafts, game
mods, photo dumps with camera names, build outputs, versioned documents, hostile names) with near-miss decoys.
76 labelled queries (52 known item, 14 attribute,
10 concept), each with graded relevant paths (2 is what the person wants, 1 a near miss).
Seed 42; the ranker is fed the fixed clock 2026-10-17 so recency is deterministic. MRR@10 counts the
first relevant path of any grade, nDCG@10 uses gain 2^grade-1, recall@50 is the share of relevant paths found in the
top 50.

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

## Latency, 1,001,303 entries (limit 50, 250 ms soft budget, 21 warm runs)

| query class | text | v1 p50 / p95 ms | fused p50 / p95 ms | hits v1 / fused | budget_hit v1 / fused |
|---|---|---|---|---|---|
| camel_hump | `texture pack` | 6.6 / 9.5 | 9.0 / 10.1 | 1 / 1 | no / no |
| concept_folder_context | `invoices 2023` | 6.2 / 9.2 | 11.5 / 19.4 | 0 / 13 | no / no |
| digits | `333333` | 9.3 / 11.9 | 10.8 / 11.9 | 1 / 1 | no / no |
| inner_substring | `recu` | 3.6 / 5.4 | 5.3 / 6.8 | 50 / 50 | no / no |
| no_match | `zzqqxx` | 2.1 / 3.3 | 3.8 / 5.0 | 0 / 0 | no / no |
| one_char | `k` | 1.3 / 2.0 | 6.1 / 8.9 | 0 / 50 | no / no |
| prefix_4 | `brec` | 2.8 / 3.8 | 19.6 / 23.3 | 50 / 50 | no / no |
| two_chars | `ka` | 1.9 / 3.2 | 7.7 / 9.9 | 50 / 50 | no / no |
| two_words_known_item | `ruquayo tanquagan 29065` | 44.3 / 63.2 | 55.2 / 64.1 | 1 / 1 | no / no |
| word | `brecunor` | 5.5 / 7.6 | 17.3 / 19.5 | 50 / 50 | no / no |
| word_and_extension | `brecunor` | 67.4 / 97.5 | 81.9 / 129.8 | 50 / 50 | no / no |

P8 (docs/46 section 9): *300 ms p95 or `budget_hit`* holds for every class on both columns. *Token and prefix queries
50 ms p95 at 1M* holds for fused ranking on the word, prefix, short, digit and camel-hump queries (every p95 is under
25 ms). Two classes are above 50 ms in both columns, and most of that is the v1 trigram passes: the three-token known item
(v1 63 ms, fused 64 ms) and the word with an extension filter
(v1 98 ms, fused 130 ms): ranked mode reads those passes no deeper than `search` does. Ranking adds a few milliseconds to a few tens (the context and token
lists), most on a four-letter prefix. No relevance query was cut by the budget at this size.

## What the token index costs

Index bytes at 1,001,303 entries: trigram 92.8 MB (92.6 B/entry, unchanged), token
42.8 MB (42.8 B/entry, `prefix='1 2 3'`, `detail=none`).

Write path, one host, `search_bench --rows 1000000` (first-scan bulk build, then a watcher-style relist of 100
folders), token triggers dropped (`--no-token-index`) against the shipped schema, two runs each, seconds:

| | publish | end_run (swap + index) | relist 100 dirs | catalog bytes |
|---|---|---|---|---|
| trigram index only (v1 write path) | 14.2, 12.9 | 35.1, 36.4 | 15.7, 24.1 | 367.1 MB |
| trigram + token index | 24.7, 10.4 | 70.9, 62.2 | 38.6, 37.5 | 393.3 MB |

So the token index adds about 27 to 35 s of `end_run` per million entries here (+75 percent over the trigram-only
write path) and the first-scan relist is slower too. This host is about 3.5 times slower than the one behind
[bench/search-v1.json](bench/search-v1.json), whose trigram-only run took 10 s, and the run-to-run spread is large (the
disk was saturated), so read the ratio, not the seconds. The queue-and-drain design (`search_dirty`, drained in the
transaction that changed the names) adds a queue insert, a join read and a queue delete per entry on top of the FTS
insert; a bulk-aware path that skips the queue for the swap is the obvious next saving and is not done here. This
cost is the price of the token tier and of the context tier built on it.

## A second scale point, 2,002,303 entries

The same harness at 2,000,000 filler files (the 2M cap of the lab's G: tier), to show how the two
columns move with size. Build: publish 26 s, end_run 80 s.

| system | MRR@10 | nDCG@10 | recall@50 | queries cut by the 250 ms budget |
|---|---|---|---|---|
| search v1 (pass order), the baseline | 0.770 | 0.725 | 0.733 | 0 |
| ranked, trigram list only | 0.770 | 0.751 | 0.733 | 0 |
| ranked, token list only | 0.809 | 0.791 | 0.773 | 0 |
| ranked, context list only | 0.263 | 0.240 | 0.211 | 0 |
| ranked, trigram + token | 0.809 | 0.791 | 0.773 | 0 |
| ranked, fused (trigram + token + context) | 0.993 | 0.981 | 0.949 | 0 |

| query class | text | v1 p50 / p95 ms | fused p50 / p95 ms | hits v1 / fused | budget_hit v1 / fused |
|---|---|---|---|---|---|
| camel_hump | `texture pack` | 6.3 / 8.8 | 7.4 / 9.7 | 1 / 1 | no / no |
| concept_folder_context | `invoices 2023` | 7.1 / 9.5 | 10.3 / 12.9 | 0 / 13 | no / no |
| digits | `666666` | 8.0 / 11.4 | 10.5 / 12.8 | 2 / 2 | no / no |
| inner_substring | `recu` | 4.2 / 5.0 | 6.4 / 7.2 | 50 / 50 | no / no |
| no_match | `zzqqxx` | 1.9 / 2.3 | 3.0 / 4.0 | 0 / 0 | no / no |
| one_char | `k` | 0.6 / 1.3 | 5.5 / 6.9 | 0 / 50 | no / no |
| prefix_4 | `brec` | 2.3 / 3.2 | 26.9 / 29.7 | 50 / 50 | no / no |
| two_chars | `ka` | 1.5 / 2.2 | 6.1 / 7.9 | 50 / 50 | no / no |
| two_words_known_item | `ruquayo tanquagan 29065` | 31.4 / 35.6 | 44.1 / 48.7 | 1 / 1 | no / no |
| word | `brecunor` | 3.8 / 5.3 | 10.8 / 13.2 | 50 / 50 | no / no |
| word_and_extension | `brecunor` | 14.5 / 17.0 | 17.4 / 25.1 | 50 / 50 | no / no |

## Not measured: the L tier (10M entries)

A 10,000,000-entry in-process build was started on this host and stopped after about 35 minutes: the staged data
alone wrote 2.5 GB to the WAL, the disk queue length stayed at 15 to 35 (it stayed there after the process was killed,
so other lanes were writing too), free space on C: fell from 47 GB to 21 GB while it ran, and it had not printed its
build summary. It would have run for hours on a saturated disk and added write load to every other lane, so it was
killed and its store deleted. **P8 at L is therefore UNVERIFIED**; the lab's real L tree
(E:, 10M files) is for scan runs and was not used. The M and 2M numbers above are the whole P8 evidence of this
lane.

## Limits of ranked mode (by design, recorded for the lane that serves it)

- **Not an enumeration.** At most 500 hits per query, offset cursor; a budget cut ranks what was read and says
  `budget_hit` with no scan position to resume. `search` (v1) keeps its keyset cursor and its guarantees untouched.
- **Bounded candidate lists.** The trigram list is read no deeper than `search` would read for the same page (a later
  pass costs far more than an earlier one); the token list reads the first 256 index matches per kind in key order,
  shorter names first, and keeps up to twice the requested depth (100 to 1,000); the context list reads up to 4,096
  entries per kind from at most 2,048 folders and keeps the same depth. A very common token on a very large catalogue is ranked within that window.
- **Camel humps and digits** are split in the index (`ThesisChapter3.docx` is `thesis chapter 3 docx`); diacritics are
  folded by the index but not by the Rust-side boosts, so a diacritic-only mismatch costs a boost, never a result.
- **Not wired to the service.** `features.federated_search` stays false and `search.query` still answers with v1;
  serving ranked results (with `score` and the match explanation) belongs to the lane that flips that flag.

## Reproduce

```powershell
cargo build --release -p loomward-lab --example search_relevance
target/release/examples/search_relevance.exe --filler 1000000 --seed 42 --out receipt.json
cargo build --release -p loomward-catalog --example search_bench
target/release/examples/search_bench.exe --rows 1000000 [--no-token-index]
```
