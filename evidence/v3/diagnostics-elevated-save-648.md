# Honest diagnostic save refusal under elevation (#648)

Windows 11, KRASPYON, 2026-10-18. Sol lane `diag-648`, branch
`fix/648-elevated-save-reason`, base `46358dfd8c89cea988b89bae2a3dd0618ccbfd13`.
Left uncommitted for the driver. Synthetic disposable fixtures only.

## Baseline and change

Read #648 and its comments through `gh issue view 648 --comments --repo Chris0Jeky/Loomward`.
The lane HEAD, `origin/main` and remote main agreed at the base above before editing.
The folder pinning and fail-closed refusal already landed in PR #640 (`ae61638`);
they are retained. The outstanding defect was the service mapping elevation to
`unsupported_target`, with no honest elevation reason in the contract or UI.

The service now checks the Windows token before inspecting a chosen target and returns
`refused`, `bytes_written: null`, `refusal: elevated_session`. The existing native handle
guard remains in force. The schema, Rust DTO, contract semantics, generated TypeScript
and UI wording agree. The UI explains that nothing was saved and asks the owner to
restart without administrator rights, preview and save again. No save capability opens.

## Regression first

All cargo commands used `CARGO_BUILD_JOBS=3` and
`CARGO_TARGET_DIR=C:/lw/target/lane-diag-648`; working directory `C:/lw/wt/diag-648`.

Before the fix, factored target validation behind a session-elevation input without
changing its behavior, then ran:

```powershell
cargo test --manifest-path C:/lw/wt/diag-648/Cargo.toml -p loomward-service an_elevated_session_refuses_new_and_existing_exports_without_touching_them -- --nocapture
cargo test --manifest-path C:/lw/wt/diag-648/Cargo.toml -p loomward-protocol diagnostics_elevated_session_refusal_roundtrips_through_schema_and_dto -- --nocapture
npm.cmd --prefix C:/lw/wt/diag-648/app run test -- tests/unit/diagnostics.test.ts
```

Service: **0 passed, 1 failed**, because an explicitly elevated session input still
accepted the target. Protocol: **0 passed, 1 failed**, because the schema rejected the
new refusal. UI: **4 passed, 1 failed**, because no elevation wording existed.

After the fix, `cargo test --manifest-path C:/lw/wt/diag-648/Cargo.toml -p loomward-service diagnostics_tests -- --nocapture`
passed **18 tests**, including the new regression, real non-elevated saves, replacement
consent, exact preview bytes and junction containment. The protocol regression passed
**1 test**, validating the schema and Rust DTO round-trip. The same UI command passed
**5 tests**. Existing successful saves are the control against blanket refusal.

## Final gates

| Exact command | Result |
| --- | --- |
| `cargo test --manifest-path C:/lw/wt/diag-648/Cargo.toml -p loomward-service -p loomward-protocol` | 436 passed, 0 failed, 1 existing ignored manual 1 GiB memory probe (protocol 56; service 379 unit + 1 integration) |
| `cargo fmt --manifest-path C:/lw/wt/diag-648/Cargo.toml --all --check` | PASS |
| `cargo clippy --manifest-path C:/lw/wt/diag-648/Cargo.toml -p loomward-service -p loomward-protocol --all-targets -- -D warnings` | PASS |
| `npm.cmd --prefix C:/lw/wt/diag-648/app run check` | 0 errors, 0 warnings |
| `npm.cmd --prefix C:/lw/wt/diag-648/app run test` | 569 passed across 49 files, 0 failed |
| `npm.cmd --prefix C:/lw/wt/diag-648/app run build` | PASS |
| `py -3 C:/lw/wt/diag-648/scripts/test_app.py` | ALL PASS, including diagnostics save/retry, Atlas, Observatory and both view lanes; timing helper 5 passed |
| `py -3 C:/lw/wt/diag-648/scripts/gen_contracts_ts.py --check` | PASS, generated file current |
| `git -C C:/lw/wt/diag-648 diff --check` | PASS |

The browser test server logged `ConnectionAbortedError` / WinError 10053 during
disconnect scenarios. The suite completed with exit 0, `ALL PASS`, and no browser
console or page errors. That test-server logging was not changed in this lane.

## Limits and handoff

The host token was measured non-elevated. Elevated-session validation uses a deterministic
session input; an actual elevated Windows process and interactive native dialog were not
run. Existing elevated-runner guards now expect `elevated_session` and still skip tests
that require a successful non-elevated write. The full workspace gate and publication
belong to the driver. `HUMAN_TODO.md` is unchanged; no owner decision is needed for #648.
