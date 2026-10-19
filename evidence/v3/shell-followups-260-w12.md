# Shell follow-ups #260 — wave 12

2026-10-18, KRASPYON, Windows 11, non-elevated token. Branch
`fix/260-shell-parser-w12`, uncommitted diff over
`f4491d943dca6867de6f727f5f92dd9c7be8c3aa`. Driver owns commits and publication.
Every cargo invocation uses `CARGO_BUILD_JOBS=4` and the dedicated
`CARGO_TARGET_DIR=C:\lw\target\lane-w12-260`.

## Item reconciliation

| Requested item | Result and test |
| --- | --- |
| Known-folder allocation scan | Replace the manual pointer scan with `lstrlenW`, using the [SHGetKnownFolderPath NUL-terminated allocation contract](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath). Keep the existing 32,768-unit rejection threshold, absolute-path validation and `CoTaskMemFree` cleanup. `known_folder_scan_uses_the_api_terminator_contract` fails before the change (0 passed, 1 failed) and passes afterward (1 passed, 0 failed). This is a source tripwire, not a reproduction of an OS allocation overread. The existing `windows_known_folder_shape_and_disposable_shortcut_read` exercises real known-folder queries. |
| Network layout | Already correct on this base and covered by merged PR #595. `network_name_offset_selects_the_documented_layout` accepts ANSI at 20 and Unicode at 28, verifies share/device/provider values, and rejects every offset in 21..27. Byte 8 is `NetNameOffset`; [MS-SHLLINK 2.3.2](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/23bb5877-e3dd-4799-9f50-79f05f938537) requires Unicode fields when it exceeds `0x14`. An ANSI-only fixture at 21..27 would violate that rule. No parser change or duplicate fixture. |
| Volume before root open | Already covered by #595: `pins_checks_volume_before_opening_the_drive_root`. Moving the check below the root open fails the test. This is structural order proof. |
| Listing-derived opens use zero object attributes | Already covered by `enumerate::native::cancellation_tests::shortcut_opens_ignore_case_and_listing_opens_stay_exact`. Mutating the listing helper's zero to `OBJ_CASE_INSENSITIVE` fails the test; changing shortcut opens to zero also fails it. No Windows production edit. |
| Other #595 overlap | Nine shared valid fuzz seeds and the 300-second mutation timeout are already present; preserved. |

## Regression proof

The new source regression was run red, then green with:

```powershell
cargo test --manifest-path C:/lw/wt/w12-260/Cargo.toml -p loomward-shell --test shell known_folder_scan_uses_the_api_terminator_contract -- --exact
```

Five compiled mutations were selected from `crates/loomward-shell/tests/mutate.py`
with `py -3 -`: the four test names above (two cases for the case-separation test).
The new NUL-scan mutation is retained in that harness. All five reached their
assertions and failed (5/5 killed), and each production file was restored in
`finally`. The runner then executes the unmodified two-crate suite with
`cargo test -p loomward-shell -p loomward-windows --all-features`.
Local logs are gitignored under `.loomward/w12-260-*.log`.

## Final checks

Commands run from this worktree (the manifest path is explicit):

| Command | Result |
| --- | --- |
| `cargo fmt --manifest-path C:/lw/wt/w12-260/Cargo.toml --all --check` | PASS |
| `cargo clippy --manifest-path C:/lw/wt/w12-260/Cargo.toml --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test -p loomward-shell -p loomward-windows --all-features` | PASS: 274 passed, 0 failed, 2 existing ignored (shell: 4 unit + 4 property + 27 integration; Windows: 231 unit + 8 property) |
| `cargo test --manifest-path C:/lw/wt/w12-260/Cargo.toml --workspace --no-fail-fast -- --test-threads=4` | PASS: 1,685 passed (including 3 doctests), 0 failed, 4 existing ignored; 84 completed test groups |
| `git -C C:/lw/wt/w12-260 diff --check` | PASS |
| `git -C C:/lw/wt/w12-260 diff --no-index --check -- NUL C:/lw/wt/w12-260/evidence/v3/shell-followups-260-w12.md` | No whitespace diagnostics; exit 1 denotes added content |

The updated Python mutation harness also passes `ast.parse` syntax validation.
No service timing failures or reruns were needed. The workspace's four existing
ignored tests are the million-row detector benchmark, the 1 GiB sparse-backup
memory probe, `durable_stop_20k_lab`, and `throughput_lab`; none was changed or run.

## NOT verified / residual risk

No hosted CI, non-Windows run or full mutation corpus. `lstrlenW` still relies on
the OS's documented terminator guarantee; it cannot make a contract-violating
unterminated allocation safe. No such invalid allocation is simulated.
The issue's existing `subst`-through-junction investigation remains outside this
task's four requested items, so #260 must not be marked fully closed.
`HUMAN_TODO.md` is unchanged; this slice adds no owner decision or file/process effect.
The running soak and its paths/processes were not touched.
