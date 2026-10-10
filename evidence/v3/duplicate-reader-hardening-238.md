# Duplicate reader hardening before wiring: #238

2026-10-10. Windows Rust target `x86_64-pc-windows-msvc`.
Issue [#238](https://github.com/Chris0Jeky/Loomward/issues/238), branch
`fix/duplicate-reader-gate-238`, base `2e35b0c` (includes #234 and #233).
Measured on the lane diff, measured before it was committed. All content fixtures and names are synthetic;
native tests use disposable files or a disposable named pipe. No personal inspection,
whole-disk scan, benchmark, effect endpoint, commit or push was performed.

## Changed

| Item | Change | Source |
| --- | --- | --- |
| 1 | `content-read` feature, enabled only by the engine; hidden low-level `inspect`; workspace source/manifest guard rejects other crate references and feature enables | `crates/loomward-windows/Cargo.toml:12`, `crates/loomward-engine/Cargo.toml:18`, `crates/loomward-windows/src/duplicate_reader.rs:103`, `crates/loomward-windows/src/duplicate_reader/tests.rs:4` |
| 2 | Remove inspection from `GrantedRoot`; engine admission calls `check_root`, refuses personal content before native opens, rereads durable class and active path/volume/file identity, and shares the existing per-root gate and revocation cancellation | `crates/loomward-engine/src/scan/mod.rs:124`, `:153`, `:503` |
| 3 | Content handles share read/delete only; metadata retains its existing sharing policy | `crates/loomward-windows/src/duplicate_reader/native.rs:161` |
| 4 | Hard-linked listed names increment `links_not_compared` and make coverage partial | `crates/loomward-windows/src/duplicate_reader.rs:84`, `:291` |
| 5 | Stream syntax in any component is `invalid_path`; repeated relative names are skipped as `duplicate_name`, never counted as multiple names of an object | `crates/loomward-windows/src/duplicate_reader/native.rs:98`, `crates/loomward-windows/src/duplicate_reader.rs:261` |
| 6 | Elevation refusal precedes even an expired budget, without opening the root | `crates/loomward-windows/src/duplicate_reader.rs:141` |

The per-root gate is held during admission and the final revocation check, not across content I/O.
Revocation requests cancellation and can return before pending reads drain.
Pending overlapped reads check the root cancellation every 10 ms while waiting and use
`CancelIoEx` plus a drain before dropping the event, buffer or OVERLAPPED. The grouping core
originally cleared equality groups on cancellation. The bug-hunt correction preserves already
verified reader groups with partial coverage and a stop reason; the engine still checks the
revocation fence again before returning evidence and refuses a revoked report in its entirety.
#233's cancel-jobs-before-watch-teardown ordering is retained.

`GrantedRoot` does not carry a durable row id. To leave the concurrently owned grant-construction
region unchanged, inspection requires exactly one matching durable path/volume/file-identity
row, and requires it active. Multiple matching rows, including a revoked row plus an active
replacement, are refused. Fixture-only grants also need an active durable registration in
the engine's configured state store. The grant-construction method and scan pipeline were
compared with the base source and match it.

## Verified: regressions and mutations

The initial reader regressions failed assertions against the unchanged reader: missing feature
gate, complete hard-link coverage, repeated names treated as links, write-sharing admitting an
existing writer, and stream syntax mapped to `changed_during_read`. Injected elevation with
zero time returned an ordinary partial budget report and failed the elevation assertion.
An engine passthrough to the old grant method reproduced the dataset mismatch, stale durable
grant and revocation-fence failures. The passthrough was replaced, not retained.

Each new test also failed with its corresponding fix removed. Fourteen distinct mutants were
run sequentially and restored byte-for-byte; the foreign caller file was removed. The stream
mutant was repeated after naming its leaf fixture explicitly `file:stream`.

| Test filter (`cargo test -p <crate> <filter> -- --nocapture`) | Temporary removal | Result |
| --- | --- | --- |
| windows: `content_reader_is_feature_gated_and_engine_only` | Remove entry-point feature/hidden attributes | Assertion failure |
| windows: same guard | Add a non-engine source importing the reader under an alias and referencing `inspect` | Source scan assertion failure |
| engine: `duplicate_inspection_checks_engine_dataset_class` | Remove engine `check_root` call | Assertion failure |
| engine: `duplicate_synthetic_grant_returns_evidence` | Ignore the active-row requirement | Stale cloned grant unexpectedly returns a duplicate group; assertion failure |
| engine: `duplicate_inspection_refuses_revoked_root_before_open` | Ignore root's revoked fence at admission | Assertion failure |
| engine: `duplicate_admission_uses_the_root_gate_and_cancellation` | Stop setting shared cancellation on `scan_cancel_root` | Cancellation assertion failure |
| engine: `duplicate_admission_rechecks_durable_class_identity_and_ambiguity` | Remove durable dataset-class recheck | Assertion failure |
| windows: `native_existing_writer_is_a_visible_sharing_violation` | Restore content write-sharing | Duplicate group unexpectedly returned; assertion failure |
| windows: `core_hardlink_exclusions_make_coverage_partial` | Remove link count/coverage update | Assertion failure |
| windows: `native_stream_syntax_is_invalid_path` | Remove colon validation | `changed_during_read` instead of `invalid_path`; assertion failure |
| windows: `core_repeated_names_are_refused_not_links` | Remove repeated-name refusal | Repeated input appears in links; assertion failure |
| windows: `native_opens_refuse_elevation_consistently` | Move elevation behind the initial budget check | Zero-time budget hides elevation; assertion failure |
| windows: `pending_content_read_is_cancelled_and_drained` | Ignore cancellation in the reader's meter | Pending native I/O stops on time budget instead of cancellation; assertion failure |
| windows: `native_modified_during_read_is_rejected` | Ignore write/change timestamps | Metadata modification goes undetected; assertion failure |

The engine admission test also holds the root gate to require `Busy`, checks that cloned grants
receive the same cancellation flag, and verifies permanent refusal after revocation. The durable
store test covers changed path, volume, file identity, dataset class and ambiguous matching rows.
The native pending-I/O test cancels a real overlapped pipe read, checks zero consumed bytes, then
successfully reads through the same handle with a fresh budget to prove it was drained and usable.
The existing during-read modification fixture now changes metadata because content writes are
correctly excluded while the content handle is held. Existing native equality controls remain green.

## Verified: gates

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` | PASS, exit 0; 475 passed, 0 failed, 0 ignored |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | PASS, exit 0; 180 passed, 0 failed, 0 ignored (engine 110 unit + 9 integration; Windows reader crate 61) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0 |
| `cargo check -p loomward-windows --no-default-features` | PASS, exit 0; no warnings |
| `cargo clippy -p loomward-windows --all-targets --all-features --target x86_64-linux-android -- -D warnings` | PASS, exit 0; non-Windows type-check/lint only |

## NOT verified

- Linux or Android runtime execution, hosted CI and an independent review were not run.
- Service/app/catalogue wiring, personal reads and file/process effects are outside this issue.
- A truly stalled filesystem driver, cloud hydration and real cloud placeholders were not exercised.
  The named pipe exercises the actual Windows pending-I/O cancellation/drain primitive.
- Python/UI/native-shell gates were not run; those surfaces are unchanged.

## Residual risk and handoff

Metadata calls and cancelled-I/O draining still depend on the driver returning; the 10 ms wait
poll is cancellation responsiveness, not a hard real-time guarantee. Equality remains point-in-time
default-stream evidence, not effect permission, reclaimability or an atomic multi-file snapshot.
The source guard enforces the workspace convention; Cargo feature unification is not a security
sandbox against external callers deliberately enabling the low-level feature.

All six requested fixes are included. Declined: service/app wiring is deferred because #238 is
the pre-wiring hardening lane. Suggested commit: `Harden duplicate reads behind active engine grants`.
