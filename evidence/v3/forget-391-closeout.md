# Remaining forget follow-ups: #391 and #434

Lane `forget-391`, branch `fix/391-forget-followups`, Windows, 17 October 2026.
Starting HEAD and freshly fetched `origin/main` both resolve to
`f308beda9ac37c79d08e4146f47c4dddfa93d922`.
Sol lane: leave all changes uncommitted for the driver; no push, PR or merge.
Synthetic SQLite fixtures only. No owner-file or process effects are enabled.

## Reconciliation against main

Both issue bodies and every comment were read with `gh issue view <n> --comments
--repo Chris0Jeky/Loomward` and the JSON form (to include the issue bodies).
The following are existing fixes, not changes made by this lane:

| Issue item | Already fixed by | Source / regression evidence at the starting HEAD |
|---|---|---|
| #391 M2: a retained same-path, nested or prefix-sharing grant causes copy deletion | `5ec41d6`, PR #562 | `forget_backups` exempts known shapes from the byte check; `migration_copy_keeps_regranted_and_nested_paths` checks all three cases |
| #391 M3: silent removal failures, no startup sweep, archived catalogue copies omitted | `5ec41d6`, `dd0cbb4`, PR #562; `2bce89d`, PR #576 | Fallible sweep, durable backup obligation, startup recovery and `health.get` visibility; service tests cover failed deletion, continued sweep, restart and automatic recovery |
| #391 crash window / lost live-store VACUUM obligation; stale catalogue cells and delayed WAL cuts | `04ca794`, PR #427; `5ec41d6`, PR #562 | State transaction writes `scrub_pending`; recovery rewrites both stores even after the catalogue purge committed; catalogue forget tests retain state/catalogue reader snapshots |
| #391 linked migration copy can mutate an external database | `5ec41d6`, PR #562 | `pin_backup` rejects aliases and pins Windows replacement; native hard-link and pin/junction regressions |
| #391 unsafe SQL table-name interpolation | `ba0b016`, PR #511 | Embedded identifier quotes are doubled; `scrub_copy_treats_table_names_as_identifiers_not_sql` retains its sentinel |
| #391 whole-copy allocation in `holds` | `5ec41d6`, PR #562 | 64 KiB streaming plus overlap; split UTF-8/UTF-16LE, EOF, missing marker, mid-stream error and bounded-read tests |
| #434 MEDIUM-1: memory-backed VACUUM main | `5ec41d6`, PR #562 | `wal_cut` temporarily selects FILE and restores views/settings, with success/failure authorizer tests; historical 1 GiB measurements in `forget-434-391.md` |
| #434 MEDIUM-2 / LOW-1: silent endless 500 ms retries; ignored marker failures | `5ec41d6`, PR #562; `2bce89d`, PR #576 | Retry intervals grow to 30 s and run between commands; durable marker, caller error and health error; conservative read failure and checked deletion |
| #434 LOW-3: sweep evidence and missing reader snapshot | `5ec41d6`, PR #562 | `the_verifier_catches_planted_damage` checks structural violations before recovery and prints each cause; missing-boundary panic has a distinct message and a child-panic negative control; `scrub_retries_progress_between_continuous_commands_and_keep_reader_snapshot` spans forget |
| #434 LOW-2: 10M-row synchronous forget measurement | `5ec41d6`, PR #562 | `forget-434-391.md` records 572.3106981 s and 2,353,012,736 B peak on a synthetic 10M-row debug run; this is historical writer evidence, not a fresh service/UI readiness pass |

## Remaining code changes

- #434 comment after #562: a v2+ copy with `root_grant` but no `object_ref`
  is unknown. Its grant IDs must not be used as live authority. The grant update
  now requires the existing known-shape check. Unknown copies still undergo the
  fail-closed byte check: forgotten bytes cause removal; unrelated copies survive
  with their path, volume and identity intact.
- #434 comment after #576: include rollback `-journal` sidecars beside migration,
  startup and archived catalogue copies. A migration's journal is left for SQLite
  until its parent is successfully scrubbed, just like its WAL. Failed scrubs
  retain the copy and sidecars with a visible pending obligation. Orphan journals
  are removed. Existing synthetic sidecar and archive regressions now include
  UTF-8/UTF-16LE journal markers.

## Verification

All commands below use
`--manifest-path C:/lw/wt/forget-391/Cargo.toml` and the environment described below.

| Command | Observed result |
|---|---|
| `cargo test --manifest-path C:/lw/wt/forget-391/Cargo.toml -p loomward-service forget_tests:: -- --test-threads=1` (before production edits) | **18 passed, 3 failed, 0 ignored, 302 filtered out**, 170.71 s |
| `cargo test --manifest-path C:/lw/wt/forget-391/Cargo.toml -p loomward-service -- --test-threads=1` (after the fixes) | **323 passed, 0 failed, 1 unchanged manual probe ignored**: 322 unit + 1 integration; 0 doctests. Unit tests 1016.99 s; integration 1.41 s |
| `cargo fmt --manifest-path C:/lw/wt/forget-391/Cargo.toml --all --check` (after the fixes) | PASS |
| `cargo clippy --manifest-path C:/lw/wt/forget-391/Cargo.toml -p loomward-service --all-targets -- -D warnings` | PASS, exit 0, no warnings; 5m 48s including shared-lock wait |
| `git -C C:/lw/wt/forget-391 diff --check` (after the fixes and final evidence update) | PASS |

The three red failures were exactly the new unknown-v2-copy assertion (the unrelated
copy's grant was rewritten), the extended state-copy sidecar assertion and the
extended archive sidecar assertion (forgotten bytes remained). Every pre-existing
selected control passed, including same-path/nested-path retention, hard-link
refusal, cleanup failure visibility, restart recovery and the held WAL reader.
Production edits were applied only after this completed red run.

All 21 forget tests pass within the full service run, including the three tests
that failed before the production edits. No unexpected failure or rerun occurred.
The existing backup byte-check, SQL-identifier quoting and native backup-pin
controls also passed. Scoped Clippy passed without warnings. Only the service
crate was changed; the workspace suite was not run, as required by the lane brief.

The first untagged service test attempt failed to compile before running tests:
`DeveloperClutterRequest` and `Command::ClutterDeveloper` were absent from the
linked protocol artifact. Both are exported by this worktree's protocol source,
but the shared `loomward_protocol-ed2cbcb40669dca1.d` dependency file omitted
`src/dto/clutter.rs`. A concurrent older worktree had built that artifact. This is
shared-pool artifact reuse, not a red regression result or a source change.

Subsequent build/test/lint commands append the unused process-only cfg
`--cfg=loomward_forget_391_lane` to the existing Rust flags. No crate source uses
that cfg (checked with `rg`); it distinguishes this lane's artifact hashes without
changing source behavior, the shared target directory, or global configuration.
All Cargo commands set `CARGO_BUILD_JOBS=4` and retain the inherited
`CARGO_TARGET_DIR=C:/lw/target/pool-b`. No target files are deleted or cleaned.
Raw tagged test output is retained in gitignored `.loomward/forget-391/`.
The driver must preserve any needed raw receipts before removing this worktree;
they are deliberately not staged or committed.

## Items left open

`HUMAN_TODO.md` is unchanged. Its answered q-9 authorises explicit forgetting and
retains an anonymised grant audit; no owner answer or readiness acceptance is inferred.

- #391 conservative shared-reference retirement, candidate-model invalidation/refit
  and volume display-name fallback remain existing behavior. Changing these is a
  product behavior change, not a repair of the copy scrub.
- Allowing `identity_changed` grants to be forgotten, or removing volume identity
  from `tier_declaration` audit/preference rows, needs an owner/product decision.
- The learning store's revoke-only `secure_delete` comment concerns
  `loomward-engine/src/learning/mod.rs`, outside this catalogue/service forget lane.
  That connection still lacks an explicit secure-delete pragma. Forget rewrites
  both live stores; this note does not claim that revoke alone erases freed cells.
- `roots.changed` events go through the engine's bounded in-memory replay buffer.
  This lane does not establish immediate erasure from replay or existing clients;
  changing replay redaction and sequence behavior remains an engine/product follow-up.
- No fresh 1 GiB / 10M-row performance probe, physical disk-full/OOM fault, hosted
  CI, Linux, or desktop/browser exercise is claimed. Full workspace verification
  and publication belong to the driver under the lane brief.

## Close-out

Changed: `handlers.rs` restricts grant scrubbing to known copy shapes and includes
rollback journals in the owned-copy sweep. `forget_tests.rs` pins unknown-copy
retention/removal and extends journal byte-removal coverage. This note reconciles
the old issue findings against the starting main and records the remaining scope.

Verified: the three tests failed for their intended causes before production edits
and passed afterwards; the full Windows service suite, formatting, scoped Clippy
and whitespace gates passed. HEAD remains the starting commit. No source edit
occurred after the successful full service test run.

NOT verified: workspace/catalogue crash-sweep reruns, hosted CI, Linux,
desktop/browser acceptance, heavy manual resource probes or physical I/O faults.
The manual 1 GiB probe is the one unchanged ignored test. `HUMAN_TODO.md` is
unchanged and no owner answer was inferred.

Residual risk: unknown copies still use the existing fail-closed byte check and
may be removed; blocked cleanup retains its visible retry obligation. The old
synchronous-forget cost and the owner/engine follow-ups listed above remain.
The shared-pool cache collision required process-only artifact isolation; the
source does not depend on that cfg. Changes and evidence are left uncommitted
for the driver; no push, PR, merge or release was performed.
