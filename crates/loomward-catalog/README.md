# loomward-catalog

Rust v0.3 catalogue for L2 (LW-005, LW-009, LW-011), architecture revision 2 and
the L2 errata in issue #117. Requires Rust 1.88; tested with Rust 1.97.1 on Windows.

`Catalog::open(state_dir, "synthetic" | "personal")` opens the dataset's two STRICT
databases, checks ownership, dataset class, supported version and `quick_check`, and
refuses UNC/device paths. Migration 2 applies the revised schema to existing v1 data.
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
listings delete absent files and hide missing child directories as tombstones.
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
Paths use one ancestor query and repeated search parents are cached. Breakdown
uses subtree-revision caching; age and active-run results are not cached.

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
