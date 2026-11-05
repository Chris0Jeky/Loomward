# Reclaim intermediate-chain regressions (#1076)

2026-11-05. Source base: maintenance branch commit
`0e7c94429920c488e9823cbd3fb6f42bbf3ed631`. This test-only follow-up addresses the
intermediate-ancestor safety gap in [#1076](https://github.com/Chris0Jeky/Loomward/issues/1076).
Production SQL, detection budgets and existing correctness tests are unchanged.

## Cases

`windows_leftover_chain_rules_hide_descendants_of_unusable_intermediates` in
`crates/loomward-catalog/tests/machine_stores.rs` starts with eight recognised
candidates, then mutates one intermediate at a time:

| Intermediate | Descendant candidates | Query path |
| --- | --- | --- |
| User `AppData/Local` | `NVIDIA/DXCache`, `NVIDIA/GLCache` | Recursive walk |
| `Windows/ServiceProfiles/NetworkService` | `AppData/Local/Microsoft/Windows/DeliveryOptimization/Cache` | Recursive walk |
| `Windows/SoftwareDistribution` | `DeliveryOptimization` | Root query |
| `ProgramData/Microsoft/Windows/WER` | `ReportArchive`, `ReportQueue` | Root query |

Each intermediate independently receives excluded flags, `listing_state='excluded'`,
and `listing_outcome='stale'`: twelve mutation/restoration cases. The descendant
rows remain current. Every case asserts the exact surviving candidate set and
retains an unrelated npm store with complete coverage and both sizes known.
Restoring the intermediate must restore the entire original candidate set.

Canonical `SoftwareDistribution` also has its own informational candidate. When
its listing becomes stale, that candidate remains with unknown sizes and a
partial report while its Delivery Optimization child disappears. Excluded flags
or state hide both. The test preserves this distinction between an observed
directory name and knowledge of its contents.

## Validation and remaining evidence

A local Python/SQLite 3.53.1 probe executed the exact checked-in
`machine-stores-root.sql` and `machine-stores.sql` against the checked-in catalogue
schema with the same synthetic fixture and detector mask. The initial eight
candidates, all twelve mutations, the unrelated control and every restoration
matched. This is SQL/fixture evidence; it does not execute Rust hydration or the
native test. `git diff --check` passes. Cargo and rustfmt are unavailable locally,
so compilation, formatting and native execution remain unverified pending CI.

Focused native command:

```text
cargo test --locked -p loomward-catalog --test machine_stores windows_leftover_chain_rules_hide_descendants_of_unusable_intermediates -- --exact
```

The existing `temp` uncertainty already says the grant is not shown to be a
volume root, including a `Windows` folder inside a project. The existing
`memory-dump` owner step already names both "System error memory dump files" and
"System error minidump files". This synthetic fixture supplies no real-machine
Disk Cleanup category witness and no multi-million-row real-catalogue timing;
those #1076 evidence questions remain separate.

## Component qualification, 5 November 2026

Head `049e74328740e2547b6e9246b9b0491524b74775`, tree
`c07bd33a3d8e152ba3731318f3550d28bc653d6f`, passed both complete attempt-1 workflows:
[PR 41956207993](https://github.com/Chris0Jeky/Loomward/actions/runs/41956207993) and
[push 41956201152](https://github.com/Chris0Jeky/Loomward/actions/runs/41956201152), each **9/9**.
Both Windows jobs ran under a non-elevated standard-user token on NTFS, child exit 0. The new
`windows_leftover_chain_rules_hide_descendants_of_unusable_intermediates` test actually passed
in each 44-test machine-store suite, with zero failures and zero ignores. Formatting, Clippy
and the remaining native/app/Python/desktop gates passed.

The same test also passed in the corrected #1146 Linux integration. Final combined Windows
qualification and main landing remain pending in #1146; this component receipt does not claim
that later integration is complete. The real-machine and real-catalogue limits above remain.


## Integrated landing — 5 November 2026

[#1147](https://github.com/Chris0Jeky/Loomward/pull/1147) is merged through the preserved ancestry
of [#1146](https://github.com/Chris0Jeky/Loomward/pull/1146). Actual main merge
`1d5aaf0fd318bdeb692e9e0ec420f05266db96a9` has exactly the final qualified tree
`29cf9220d575824e1e7e17b3ccf5d0ad8492de42`.
Integrated head `c0ab2262bcfaf5c58fea09e181ca12637a80b72a` passed
[PR 41960178507](https://github.com/Chris0Jeky/Loomward/actions/runs/41960178507) and
[push 41960173043](https://github.com/Chris0Jeky/Loomward/actions/runs/41960173043), 9/9 jobs each.
The chain regression actually passes in each 44-test machine-store suite on Linux and standard-user
Windows. Together with the earlier complete standalone qualification, this closes the test gap,
while #1076 remains open for the separately identified real-machine/performance/design work.
