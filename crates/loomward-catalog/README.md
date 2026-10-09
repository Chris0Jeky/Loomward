# loomward-catalog

Rust v0.3 catalogue for L2 (LW-005, LW-009, LW-011). Requires Rust 1.88 or newer because of
the approved `time` dependency; measured here with Rust 1.97.1 on Windows.

`Catalog::open(state_dir, "synthetic" | "personal")` opens a local dataset pair. It preflights
both files, runs `quick_check`, migrates the exact doc 41 section 6 STRICT schema, and backs up
`state.db` with `VACUUM INTO` before migration. Foreign/nonempty unowned and newer databases
are refused. A corrupt derived catalogue is preserved under `catalog.corrupt-*.db`, including
sidecars, and recreated; precious state corruption is refused. UNC/device paths are refused
before opening, and existing database targets are canonicalised and checked too. Interrupted
runs become failed/stale and their committed observations are rolled up as incomplete.

The engine must own one `Catalog` per dataset and its instance lock (ADR-V3-01). All writes go
through its single `Writer`; the engine should retain at most four `Reader` handles (ADR-V3-06).
Opening another `Catalog` for an actively served dataset is not a supported recovery operation.
The crate does not grant filesystem access or enumerate any supplied display path.

## Writer

`RegisterRoot` records observations already admitted by the native grant layer. `BeginRun`
requires an active grant and no other running scan for it. `DirListing` accepts a complete
directory observation, not chunks; `DirFinal` accepts the engine arena's subtree totals.
`EndRun` marks hard links within the grant, rolls up accounting, and checkpoints the WAL.
`Barrier` waits for prior messages. `send` returns a receipt acknowledged after commit;
`call` waits for that receipt. Dropping the catalogue drains the writer and joins it.

Queue: 64 messages, 200,000 entry reservations; one listing may contain up to 2,000,000
entries and, above 200,000, reserves the whole entry quota exclusively. That oversized-listing
exception is bounded but exceeds the ordinary queued-entry memory estimate in doc 41.
Adjacent small listings coalesce up to 50,000 reserved rows or 250 ms, with a savepoint per
listing. A malformed listing rolls back as a unit without erasing valid neighbours. Readers
see complete committed listings only. IDs are monotonic and retained for unchanged names,
identity matches and renames. Already-present directory identities can reparent across listings; descendants keep their rows
and depths, and cycles are refused. Missing directories cascade; if an identity is rediscovered
after its source row was deleted, its row is recreated and durable state still binds by
`object_ref`. Sums are signed-64-bit-safe;
unknown allocation stays NULL and hard links are flagged, never described as reclaimable.

## Reader and L8 mapping

Read methods: `slice`, `slice_with_overlay`, `children`, `path`, `inspect`, `search`, and
`breakdown`. Each runs in a read transaction. Projections mirror the contract field-for-field:
`TreeSlice`, `SliceNode`, `EntryRow`, `NodePath`, `NodeDetail`, `Breakdown`, and `EntryPage<C>`.
Tests validate real outputs against `contracts/v3/view-service.schema.json`. Byte fields are
decimal strings, display paths are `{text,truncated}`, and missing values remain nullable.

`NodeKey` and `nd_dir_*`/`nd_file_*`/`rt_*`/`vo_*`/`cl_*` projection references are **internal**,
not wire-ready session IDs or authorization. L8 must recursively replace them with its
session-bound opaque IDs and sign the typed children/search cursors before serialization.
Do not deserialize a browser cursor directly into these internal structs. Children cursors
bind directory, listing revision, sort and basis; search cursors bind filters and every
selected root's revision. Listing, aggregate finalisation, rollup and recovery invalidate
affected revisions. Unknown totals on pages are `None`, not a guessed zero.

Slices expand a max-heap of directories, return pre-order parent indexes, and subtract visible
children from materialised totals to form `other` without enumerating the remainder. Immediate
directory counts not materialised in the schema remain nullable. Allocated-file ordering is
labelled approximate; live results are labelled approximate_live and accept the L7 arena
overlay explicitly. The engine must supply that overlay for current live subtree totals.

Search and breakdown use SQLite progress handlers with caller-supplied VM-operation budgets;
handlers are removed on success and failure. Search reports `budget_hit`; interrupted
breakdowns report incomplete coverage and put unaccounted observations in `unknown`.
Complete extension/family breakdowns have a bounded 64-result generation cache. Age bands
are not cached because the clock moves. Proactive root/depth-1 cache warming belongs to the
engine's finalisation integration; it can call this same API after `EndRun`.

Meaning is pending until L14 supplies its thread, suggestions are absent, and every inspected
identity says `authorises_effects: false`. Native grant/identity verification, permission to
disclose/train, and operating-system effects are outside this crate.

## Proof

```text
cargo test -p loomward-catalog
cargo clippy -p loomward-catalog --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
cargo run -p loomward-catalog --release --example bench -- --rows 1000000
```

The benchmark synthesises rows in-process in a `TempDir`, checks exact accounting against an
independent oracle, closes all handles, explicitly removes the directory, then writes
`evidence/v3/bench/catalog-1m.json`. It measures all schema indexes, insertion including
generation/backpressure, 21 warm query runs, serialization, and checkpointed database size.
It does not enumerate disks. P6/P14 are measured at 1M, not their 10M target; HTTP, engine
memory, cold-cache performance and personal datasets are unverified. P4 misses its hypothesis;
the initial miss remains in `evidence/catalog-1m-before-optimization.json`.
