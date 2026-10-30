# loomward-catalog

Rust v0.3 catalogue for L2 (LW-005, LW-009, LW-011), architecture revision 2 and
the L2 errata in issue #117. Requires Rust 1.88; tested with Rust 1.97.1 on Windows.

`Catalog::open(state_dir, "synthetic" | "personal")` opens the dataset's two STRICT
databases, checks ownership, dataset class, supported version and `quick_check`, and
refuses UNC/device paths. Migration 2 applies the revised schema to existing v1 data;
migration 3 adds the pending reference journal without changing existing bindings or instances.
Migration 4 adds a nullable listing outcome; existing observations and references stay intact.
Derived catalogue corruption is archived with its sidecars and rebuilt. Precious
state corruption is refused. `main.synchronous=NORMAL`, `st.synchronous=FULL` and
foreign keys are enabled. No transaction writes both persistent files.

Precious state gets a `VACUUM INTO` backup at startup (seven owned startup backups
kept) and before migration (migration backups retained). V1 state tables remain as
`legacy_v1_*`: object references become unresolved/retired, and compatible taxonomy,
collections and student rows are copied. V1 labels, teacher disclosures and other
incompatible history remain archived; the crate does not invent their missing v2
provenance. Serving that archived history needs a separate adapter. Existing v1
grants are copied to state before rebuilding the catalogue; rebuild never grants
new access or revives a revoked grant.

The engine owns one `Catalog` and its dataset lock (ADR-V3-01), and retains at most
four `Reader` handles. The crate records admitted observations; it does not enumerate
paths, verify OS grants, open user files or perform effects.

## Cloud residency observations (LW-120 slice 1)

`cloud_state(Option<u32>)` is a pure classification of already-listed Windows
attributes. Recall-on-data-access (`0x00400000`), recall-on-open (`0x00040000`)
or offline (`0x00001000`) takes precedence over pin preferences and produces
`CloudOnly`: **not fully local**, possibly partially hydrated. This includes
offline storage that is not a cloud provider. Pinned (`0x00080000`) without
recall/offline evidence gives `Pinned`; conflicting pinned/unpinned preferences
give `Unknown`. Unpinned (`0x00100000`) alone is an eviction preference, not
proof of eviction: an unpinned reparse entry without recall/offline flags gives
`Local`. Other ordinary attributes give `Local`; unclassified reparse
or sparse entries, zero/missing attributes, and all non-Windows hosts stay
`Unknown`. `Pinned` records a preference, not confirmation of completed hydration.
These are scan-time observations, never an assurance that a later open is safe.
The bit meanings follow [Microsoft's file attribute constants](https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants).

`Reader::cloud_summary(directory_id, work_budget)` queries an active directory's recorded file
entries recursively in one catalogue snapshot. Each of its four state buckets
contains file-entry count, logical bytes and independently observed allocated
bytes (`None` if any contributing entry lacks allocation). Empty buckets have
known zero allocation. Placeholder allocation is never assumed to be zero, nor
inferred from logical size. Hard links count per name; these are not unique or
reclaimable totals. There is **no file-flag filter**: files marked `cloud_placeholder`,
`reparse_not_followed`, `excluded_state_dir`, `sensitive_name` or `protected_context`
are counted when their containing directory is included, even where normal scan
aggregates omit them. A hydrated reparse placeholder without recall/offline or
pin/unpin evidence lands in `Unknown`; pin/unpin cases follow the classifier above.
Absent directory subtrees and descendants of excluded/denied or exclusion-flagged
directories are omitted, including retained descendants after a denied re-listing.
The effective listing outcome is used alongside the stored listing state. `partial`
records incomplete/stale/live scan coverage, including skipped denied subtrees;
totals describe recorded files, not all remote provider contents.
The query observes active grants and existing reader deadlines. Its caller-supplied
SQLite VM-step budget refuses exhaustion with `ResourceBudget` instead of publishing
a truncated total. Deadline cancellation returns `Cancelled`; an unrelated SQLite
interrupt remains a SQL error.

The suggested default is `Reader::DEFAULT_CLOUD_SUMMARY_STEPS` (32,000,000).
On Windows, `cloud_query_vm_steps_per_file` measured the production SQL on flat
synthetic subtrees using `StatementStatus::VmStep`: 1,000 files took 27,102 steps;
10,000 took 270,102; 100,000 took 2,700,102 (27 steps/file plus 102 fixed steps).
That implies about 74,000 files at the former 2,000,000-step budget for this revised
query, or about 1.18 million at the suggested default. Before checking the effective
listing outcome, the query took 21 steps/file plus 80. These are flat-query
estimates, not universal file limits: root/coverage checks consume budget too,
directory traversal adds work, and a reader deadline can stop the query sooner.

No migration is needed: Extended/Both enumeration already supplies `FileAttributes`,
`EndOfFile` and `AllocationSize` (`crates/loomward-windows/src/enumerate/mod.rs`);
the Find fallback supplies attributes and logical size, with unknown allocation
(`enumerate/native.rs`). The engine copies them into `scan::Entry`; the service
copies them into `Observation` (`crates/loomward-service/src/sink.rs`); the catalogue
writer persists `file.attrs`, `file.logical` and nullable `file.allocated`.
The new query reads these existing columns; old stores need no backfill.

This slice stops at the catalogue API. No protocol DTO, generated app type or UI
changes are made. Provider identity, sign-in, presence and connection health
(including absent/offline provider status) **remain unknown**: filesystem OFFLINE
is file availability evidence, not provider health. No provider API, network,
content open, sign-in or metadata refresh is added. A later provider-health/UI
slice must keep this distinction visible.

Proof uses synthetic SQLite observations, reopen tests, and encoded Extended/Both
enumeration records with a native-open recorder. Recall flags require provider/
kernel support; the tests do not pretend an ordinary file is a real placeholder.
Real OneDrive/Dropbox/Google Drive hydration and directory-provider behaviour
remain unverified. Classification and catalogue queries never open scanned paths;
the existing no-recall directory traversal guards are unchanged.

## Developer clutter advice (LW-119 first slice)

`Reader::developer_clutter(root_id, cap)` reads recorded metadata in one snapshot,
under the existing active-grant, deadline and SQLite work-budget rules. `cap` is
1–200. The result ranks known logical aggregates largest first, then unknowns,
with explicit `partial` and `truncated` flags. It never scans a project or reads
manifest contents. Candidate and anchor `NodeKey`s are internal references.

Rules version 1 requires an adjacent **recorded file**, using conservative,
case-sensitive conventional names:

| Manifest | Direct sibling output directories |
|---|---|
| `Cargo.toml` | `target` |
| `package.json` | `node_modules` |
| `pyproject.toml`, `requirements.txt` | `.venv`, `venv`, `__pycache__` |
| `*.csproj`, `*.sln` | `bin`, `obj` |
| `build.gradle`, `build.gradle.kts` | `build`, `.gradle` |

`go.mod` alone has no universal local output directory, so there is no Go output
rule. Names, extensions and sizes alone never identify a candidate. Source and
dataset directories outside these explicit pairs are not listed. Multiple
manifests do not duplicate a directory; nested outputs inside another candidate
are omitted to avoid double counting. Excluded, denied, tombstoned, protected,
lossy and reparse observations (including ancestors) cannot anchor candidates.
Retained files missing from an incomplete relisting cannot anchor them either.

Each candidate carries its manifest, provider, explanation and regeneration hint.
The cost class is `unknown`: metadata does not establish dependency/toolchain
availability, offline rebuilds, active builds or unique contents. These are
**candidates for review, never a claim that every contained file is recreatable**.
Logical and allocated sizes come only from valid, complete catalogue aggregates;
partial/unscanned/stale or currently scanning sizes stay `None`. Unknown allocation
also stays `None`; a known empty directory remains zero. Bytes are observed sizes,
not a promise of reclaimable space.

This slice is a library API only. A service command requires coordinated routing,
DTO/schema/example, TypeScript generation and mock-transport work; no wire
contract or capability flag is changed here. Follow-ups are that service surface,
measured regeneration costs (LW-087), model lifecycle (LW-086) and stale worktrees
with lease and dirty-state evidence (LW-076). No file/process effect is opened.

The deterministic synthetic project lab in `tests/developer_clutter.rs` exercises
the SQLite representation, including ordinary writer publication and aggregate
calculation. Run `cargo test -p loomward-catalog --test developer_clutter`.
The explicit 1M-row synthetic detector benchmark (including query plans and VM-step
counts) is `cargo test -p loomward-catalog --test developer_clutter million_row_output_interiors_do_not_increase_detector_work -- --ignored --nocapture`.

## Writer

All writes use one actor. `RegisterRoot` returns distinct `grant_id`, derived
`root_id` and root `dir_id`. Native identity bytes are eight or sixteen bytes;
ReFS requires sixteen bytes for uniqueness, reparenting and durable references.
A ReFS eight-byte observation remains path-only. `BeginRun` requires an active
grant and permits one active run per root. `RevokeGrant` commits to precious state
first, then fences subsequent observations and marks the run cancelled.

`StageChunk` appends contiguous chunks of at most 16,384 entries. `ListingDone`
publishes the whole directory in one transaction, decoding one chunk at a time;
staged rows are invisible beforehand. A listing is capped at two million entries.
An incomplete outcome only upserts, retaining unseen entries as stale. Complete
outcomes after a rejected chunk are downgraded to partial for that run and directory.
Denied, excluded, cancelled and other validated incomplete outcomes survive
projection and restart. Complete listings delete absent files and hide missing
child directories as tombstones.
Only a complete root traversal may sweep tombstones; targeted runs retain them.
Identity matches precede names, so directory moves retain descendants in either
listing order and dirty both ancestor chains. Depths are repaired at finalisation.

`DirListing` is a convenience for a single small listing with the same publication
rules. In-flight memory uses one 64 MiB byte budget, with 32 MiB reserved for writer
scratch and 32 MiB available to producers. Call `reserve_bytes` **before** constructing
an engine chunk, then `send_reserved`; late-reserving `send` is a convenience for
already-owned small inputs. Reservations cover vector capacities, strings and
serialization/decode scratch. Every quota wait and send observes cancellation and
writer failure. Queue capacity is 64; small commands coalesce within a row/time bound.
Receipts are acknowledged only after commit and release their reservation first.

A publication advances `catalog_rev`, dirties subtree revisions, and advances
`listing_rev` only when names or membership change. Ancestor invalidation coalesces
the union of affected chains per transaction. `DirFinal` includes `input_revision`;
it is accepted only for the active run, matching dirty run and exact dirty revision,
and publishes `agg_valid_rev=input_revision`. `EndRun` and recovery use one checked
bottom-up directory pass, rather than per-depth table scans. Unknown allocation,
entry totals and unique-object totals remain separate; none means reclaimable space.

`ObjectReference` requires native NTFS/ReFS identity and observed creation time.
Observed deletion or creation-time change retires the binding. Even reuse of the
same identity and creation time creates a new incarnation. `ReconcileReference` is
the explicit engine-controlled operation for restoring a selected old reference;
no automatic attachment occurs after continuity is lost.

Retirements and surviving hard-link rebindings first commit as pending intents in
`state.db`, leaving effective bindings and collection membership intact. The catalogue
publication commits its random token in `meta` in the same main-only transaction.
A separate state-only transaction confirms matching intents and removes the journal.
Failure and startup recovery discard intents without a matching token and catalogue
instance; recovery runs before any derived repair writes and is idempotent. A failed
confirmation closes the writer with the journal intact for recovery to retry.

## Reader and engine integration

Read methods are `slice`, `slice_with_overlay`, `children`, `path`, `inspect`,
`search` and `breakdown`. Each uses one read transaction. Committed reads repair
dirty inactive roots and refuse inconsistent aggregates during an active run.
Live slices require one complete arena snapshot and say `provisional_live`.
Live `other` clamps increment `Reader::live_clamp_count`; committed arithmetic
never silently clamps. Unscanned generations stay null.

Logical and allocated children use their deterministic keyset indexes, with
unknown allocation after known zero. Name/modified sorts are bounded to 10,000
direct children. Children cursors bind catalogue instance, anchor, `subtree_rev`,
sort and basis. Search binds the instance, `catalog_rev`, full filter and resumable
scan watermark; a budget hit can return a continuation even without matches.
Search streams each pass once per page under the work/deadline guard; rejected hits advance
the same cursor without restarting FTS every 128 candidates. Paths use one ancestor query;
search ancestry uses a bounded recursive query and caches repeated parents. Breakdown
uses subtree-revision caching; age and active-run results are not cached.

Synthetic 1M/10M query-cost measurements: [#552 evidence](../../evidence/v3/search-n1-552.md).

`search_ranked` (LW-113 F2, off by default: it needs the token index, which
`Catalog::open_with(.., &CatalogOptions { ranked_index: true })` builds; without it the call
returns `Error::RankedIndexDisabled` and the store holds no ranked-index object, see #520)
answers the same request by weighted reciprocal-rank
fusion of three candidate lists: the trigram passes above, the token index
(`search_token`, name-tuned tokens: separators, camelCase, letter/digit
boundaries, the extension; `search_dirty` queues changed keys and the writer drains
it in the commit that changed them) and a context list (entries under folders whose
names, with their parents', carry the query). Exact, prefix, token-start and
(with a caller clock) recency add boosts; ties break by index key. Pages are cut
from one pool fixed by the query, filters, options and page size (the cursor binds
them), at least 100 hits deep and at most 500 answered; it is not an exhaustive
scan: a budget cut ranks what was read and says `budget_hit` without a resume position. `search` is
unchanged. Evidence: `evidence/v3/search-relevance-f2.md`.

Projections are checked against the JSON Schema. Byte counts are decimal strings;
unknowns remain nullable. `NodeKey` and projected references are internal, not
session wire IDs or permission. L8 must authenticate the catalogue instance,
kind, row ID and `born_run` and sign typed cursors. L7 supplies revisions and arena
snapshots. Meaning remains pending for L14; identities never authorise effects.

## Proof

```text
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p loomward-catalog --release --example bench -- --rows 1000000
```

The benchmark generates synthetic metadata in a disposable temporary directory,
uses staged publication with the schema indexes present, checks exact totals against
an independent oracle, removes the directory and writes the
[receipt](../../evidence/v3/bench/catalog-1m.json). P4 remains below 250,000 rows/s;
see [HANDOFF.md](HANDOFF.md) for the comparison and measured writer timings.
P6/P14 are at 1M rather than 10M. Cold-cache, HTTP, peak process memory, physical
power-loss durability and native engine/service integration remain unverified.

## Machine store advice (rules v2)

`Reader::machine_stores(root, cap)` recognises machine-level build outputs, package caches,
model stores, virtual disks, worktree-shaped directories, the Recycle Bin and Windows-managed
space from catalogue names and structure only. It reads one transaction without repairing
aggregates, opening store paths or running tools; owner commands and relocation settings are
display text. A recognised directory shields its descendants from double counting. The walk
also prunes `node_modules`, `.git` directories and the manifest/output pairs listed for v1
above; project-output detection stays separate. Any recorded parent `Cargo.toml` prunes
`target` and vetoes a shared Cargo target, even if the manifest is stale or shielded.
Shared Cargo targets require a complete parent listing, so a granted root with no observed
parent cannot match. Other v1 pairs use observed manifests, including the same-run requirement
under incomplete listings. pnpm requires `store`, `.pnpm-store` or a parent named `pnpm`,
plus a `v<digits>/files` child. Results use a 2,000,000-step budget, the reader's
deadline and a 1..=200 cap. A non-recursive query first collects the root's Recycle Bin and
system-managed candidates, including direct children of its `Windows` directory. If the
subsequent walk exhausts the budget, these candidates still return with `partial: true`
and `budget_hit: true` (normally false). Exhaustion before the root query completes returns
`Error::ResourceBudget`; deadline expiry returns `Error::Cancelled`. Completed results are
ordered by known logical size descending, then node id (directory before file on equal ids).
Partial, invalid or mid-scan directory sizes and unknown allocations
stay null; a file uses its own observed sizes, also null during a scan. Observed allocation is
never a reclaimability estimate. The `agent_worktree` class means only that a `.git` file
marks a linked worktree, a submodule or a separate-git-dir checkout; names alone cannot tell
which. Check `git worktree list` and `git submodule status` in the owning repository before
removing anything; the marker does not prove ownership or disposability.
Rules v4 add names-only Windows leftovers (classes `windows_leftover`, and `windows_manual_leftover` for crash dumps and driver extracts: `Windows.old` and its
siblings, memory dumps, error reports, `Temp`, `CrashDumps`, shader caches, driver extracts and
Delivery Optimization) and the `Package Cache` and `Windows\Installer` stores as `system_managed`
(see docs/52 section 3). Root-relative paths are matched by the root query; `AppData` paths by their
parent chain in the walk.
Synthetic recognition, decoy, shielding, coverage and row-checksum tests are in
`tests/machine_stores.rs`.
