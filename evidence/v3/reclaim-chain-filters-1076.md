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
