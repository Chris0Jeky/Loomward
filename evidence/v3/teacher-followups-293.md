# Teacher confinement follow-ups: #293 and #176

17 October 2026, Windows 11 Home, KRASPYON. Uncommitted worker slice based on
`3edc476d4b2903d3ef8cab52138200012a0db27e`. The relevant teacher, broker and sandbox
sources also match `origin/main` at `f308bed`. No provider request, profile registration,
download or confinement rerun.

## Item reconciliation

- **#176, partial child report:** already fixed by `5faab7d` (PR #291). The child writes
  `.partial`, then renames it to the final report. A test now exercises that publication path.
- **#176, dropped log read/UTF-8 errors:** already fixed by `5faab7d` (PR #291).
  `public_log` records `read_error` with the error kind and code. New tests cover missing
  and non-UTF-8 files. Neither item required a second implementation.
- **#293.1, interruption:** scoped Ctrl+C/Ctrl+Break handling and serialized measurement/recovery;
  marked disposable roots can be swept by `--delete-profile`. Root/marker reparse points and
  unmarked legacy roots are retained and counted as unverified. Recovery reports incomplete
  cleanup as failure. Console close/kill/power loss can still bypass cleanup, as documented.
- **#293.2, stage/provenance:** specified the compiled stage ceiling, digest binding and independent
  per-item lab-manifest validation in the proposed teacher ADR. Runtime work remains open: the
  teacher request builder/runner is not implemented; the engine still returns unavailable for
  synthetic runs and refuses personal runs. The current `loomward-broker chat` implements the
  separate Ask contract, not this proposed teacher contract. Provider qualification and adoption
  of the proposed ADR remain decisions for the owner/driver. No teacher gate is opened here.
- **#293.3, low-integrity control:** only the granted lab inherits low integrity. The ordinary
  control executable, working directory and stdio live in the medium-integrity root.
- **#293.4, profile race:** `dfa73a6` already protected `ERROR_ALREADY_EXISTS`. Other failed
  creation HRESULTs still invoked deletion. Cleanup now deletes only after successful creation.
- **#293.5, log redaction:** Windows ordinal case-insensitive matching covers long and native
  short-path aliases, slash variants and JSON escapes. Tests also cover Unicode casing.
- **#293.6, stale claims:** the proposed ADR now records the reconciled q-8 answer and accurately
  limits the filesystem claim to the synthetic directory whose ACL was actually measured.

## Verification

The initial standalone Rust example harness ran **7 passed, 3 failed** before the fixes. Failures
were the non-`ALREADY_EXISTS` profile race, low-integrity inheritance and path redaction. The new
console/recovery tests initially failed to compile before their missing helpers were implemented.
The final harness ran **14 passed, 0 failed, 0 ignored**. It compiles the current example with
`rustc --test --edition=2021`, its existing dependency artifacts and the declared Windows API
features, then runs the executable with `--test-threads=1`; the local helper is
`.loomward/teach-293-check.py` (gitignored). This is distinct from the required Cargo checks.

- `cargo fmt --manifest-path C:/lw/wt/teach-293/Cargo.toml --all --check`: passed.
- `git -C C:/lw/wt/teach-293 diff --check`: passed.
- Local links in the two edited documents: **10 passed**.
- Standalone `clippy-driver -Dwarnings --test --edition=2021` over the same example and
  dependencies: passed. Its first pass found the existing test module before production items;
  the module was moved to the end without changing assertions, then Clippy and all 14 tests passed.
- `cargo test --manifest-path C:/lw/wt/teach-293/Cargo.toml -p loomward-lab --all-targets` and
  `cargo clippy --manifest-path C:/lw/wt/teach-293/Cargo.toml -p loomward-lab --all-targets -- -D warnings`:
  **UNVERIFIED: blocked on the shared build-directory lock.** The earlier focused Cargo test
  waiter was stopped after more than 25 minutes without building. At handoff the full crate
  check and standalone Cargo Clippy also remained blocked; this lane's idle waiters were stopped
  to prevent late builds after handoff. No other lane's processes or target artifacts were changed.
  The driver must run the two commands above before accepting the slice. All Cargo invocations
  used `CARGO_BUILD_JOBS=4` and the supplied shared `CARGO_TARGET_DIR=C:\lw\target\pool-a`.

## NOT verified and residual risk

No live AppContainer matrix, real console-close/kill recovery, provider qualification or personal
metadata disclosure. The console test checks callback dispositions and native registration, not an
external Ctrl+C delivery. The ACL regression pins the descriptor supplied to Windows, not a new
confinement measurement. Older roots without ownership markers require separate inspection.
`HUMAN_TODO.md` is unchanged; its provider/authentication and personal-teacher canary gates remain.
