# Duplicate candidate count and cache (#1104, #1105)

Base: local `4db7f22494b7ba330945e7ba7df752e88da6ea55`, verified remote main
`3f5aaf2b9dd6a9c49d1c0471f4390deb6df5d10a`.

1. Add native regressions for an instance change with identical generation/revision,
   live-run and invalid-aggregate guards alone, root/floor separation, interrupted
   counts, hardlink flag coupling, and grouped-count overflow. Keep a tests-only
   patch for the coordinator's hosted red run. Use RegisterRoot's actual grant id.
2. Count grouped eligible sizes once, summing integer group counts and integer byte
   sums without multiplication that could silently promote an overflow to REAL.
   Preserve the shared visibility, flags, floor, unknown and error rules.
3. Include the catalogue instance in the cache stamp, retaining the existing guards
   and bounded cache. No schema migration, index, file effects or shared CI edits.
4. Measure the exact production SQL over the checked-in schema, indexes and visible
   view on synthetic 130k-file one-size and 20k-size shapes. Record first execution
   separately from repeat SQL execution; neither is a native cache-hit measurement.
5. Retain the ignored native measurement for hosted/Windows follow-up. Report Rust
   gates as unverified locally because the toolchain is unavailable; coordinator
   owns publication, full integrated checks and a fresh implementation review.
