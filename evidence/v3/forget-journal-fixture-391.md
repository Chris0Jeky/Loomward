# Real rollback-journal fixture: #391 / #632 review

18 October 2026, KRASPYON, Windows 11. Sol lane `forget-fixture`, branch
`fix/391-journal-fixture`. Starting HEAD and freshly fetched `origin/main`:
`76b4868f1a26599a9696c0740cd4b243e2e5fb8c`. Left uncommitted for the driver.

## Reconciliation and change

Read #391's body and comments with `gh issue view 391 --comments --repo
Chris0Jeky/Loomward` and the JSON body form. The older findings were reconciled in
[forget-391-closeout.md](forget-391-closeout.md): #562/#576 cover exact-grant
retention, pending cleanup and recovery; #511 quotes SQL identifiers.
The specific #632 follow-up is still real: the sidecar test wrote arbitrary
UTF-8/UTF-16 marker bytes to the migration copy's `-journal`.

Production ordering is **already fixed by `7c494e4`, PR #632**, merged as
`ae2012c49bab12be5581f00b0152d33189b1391d`: migration sidecars with an existing
parent are deferred to `scrub_copy`, and only removed after SQLite closes.
No production change is needed or retained by this lane.

The replacement fixture runs the exact test in a child process, spills an
uncommitted kept-grant update with a one-page cache, and exits without running
SQLite's rollback destructor. Preconditions check the hot journal magic,
the committed kept-path bytes in that journal, and uncommitted bytes in the
database itself. After `roots.forget`, the test requires the migration copy to
survive, the forgotten grant to be scrubbed, the kept grant's committed path to
be restored, all sidecars/orphans to be removed, and `integrity_check` to be `ok`.
The existing orphan UTF-8/UTF-16 and stale WAL/SHM coverage is retained.
All files and names are disposable synthetic fixtures.

## Direct red/green evidence

Every cargo command uses `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:/lw/target/lane-forget-fixture`, from
`C:/lw/wt/forget-fixture`.

1. Before replacing the fixture, added the hot-journal header assertion to
   `forgetting_removes_state_copy_sidecars_including_orphans`. Ran:
   `cargo test --manifest-path C:/lw/wt/forget-fixture/Cargo.toml -p loomward-service forgetting_removes_state_copy_sidecars_including_orphans -- --exact tests::forget_tests::forgetting_removes_state_copy_sidecars_including_orphans --nocapture`.
   **0 passed, 1 failed, 351 filtered out**: the existing file starts with
   `ForgetSe`, not SQLite's hot-journal magic.
2. After the replacement, ran:
   `cargo test --manifest-path C:/lw/wt/forget-fixture/Cargo.toml -p loomward-service forgetting_recovers_migration_journals_before_removing_sidecars -- --exact tests::forget_tests::forgetting_recovers_migration_journals_before_removing_sidecars --nocapture`.
   **1 passed, 0 failed, 351 filtered out**; integration target: 0 selected.
3. Negative control: temporarily remove `copy.with_extension("db-journal")`
   immediately before SQLite opens in `scrub_copy`; repeat command 2.
   **0 passed, 1 failed, 351 filtered out**, specifically at the ordering assertion:
   actual `("forgotten", "UncommittedMigrationPath")`, expected
   `("forgotten", "KeepServicePathBeta")`.
   Removed the mutation and proved `git diff --exit-code --
   crates/loomward-service/src/handlers.rs` is empty.

## Final checks and limits

`cargo test --manifest-path C:/lw/wt/forget-fixture/Cargo.toml -p loomward-service -- --test-threads=3`
passes: **351 unit + 1 integration = 352 passed, 0 failed, 1 existing manual
probe ignored, 0 doctests**. Unit tests took 132.14 s; integration 0.48 s.
No unexpected failure or rerun occurred; the restored production code passed the
new journal assertion in this full run.
`cargo clippy --manifest-path C:/lw/wt/forget-fixture/Cargo.toml -p loomward-service --all-targets -- -D warnings`
passes, exit 0, no warnings (27.07 s).
`cargo fmt --manifest-path C:/lw/wt/forget-fixture/Cargo.toml --all --check`
and `git -C C:/lw/wt/forget-fixture diff --check` pass.

NOT verified: Linux, hosted CI, physical power loss, desktop acceptance, the
workspace suite (reserved for the driver), and the existing ignored 1 GiB memory
probe. This proves interrupted-process rollback and observable cleanup ordering,
not power-loss durability. No personal files, process policy, product gates or
shared status documents were changed. `HUMAN_TODO.md` is unchanged; #391's
identity-change/audit-retention product decisions remain open for the owner.
