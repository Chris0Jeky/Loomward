# Coverage cancellation and worker accounting (#1108)

Base: GitHub main `3f5aaf2b9dd6a9c49d1c0471f4390deb6df5d10a`.

1. Pin the existing service API's missing worker reservation with a held synthetic
   read. Publish this tests-only revision for hosted Windows behavioral red evidence.
2. Extend native tree-read cancellation to observe the job and both root flags
   directly. Preserve durable admission and the ordered final root gate.
3. Run coverage in its existing job worker, holding its reading guard and one shared
   scan-worker reservation until actual I/O returns. Cancellation stays requested
   while an outstanding call drains; no helper read thread or premature terminal state.
4. Pin global coverage conflicts, permit exhaustion/release, phase cancellation,
   pending native I/O drain, result suppression, and explicit native refusal accounting.
5. Update the CLI caller and the coverage behavior note. Run all available checks;
   exact-head hosted Rust/Windows CI and fresh review remain required before merge.

No wire/schema changes, new effects, gate changes, dependencies, or progress contract
in this slice. Synchronous metadata calls still depend on their filesystem driver.
Rust tooling is absent locally: missing-tool failures are UNVERIFIED, never test red
or green. New cancellation APIs cannot be expressed by the pre-change signature;
those tests need staged implementation or a separately reported red limitation.
