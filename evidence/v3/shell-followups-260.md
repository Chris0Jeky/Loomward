# Shell follow-ups #260

2026-10-17, Windows 11, branch `fix/260-shell-parser`, uncommitted diff over
`00bb610b4a50c8ab5449cd69efd507ff0ad03437`. No commit, push or PR in this Sol lane.
`CARGO_BUILD_JOBS=4`; inherited shared `CARGO_TARGET_DIR` retained throughout.

## Issue reconciliation

| Item | Result |
| --- | --- |
| Known-folder NUL scan | Not a reproduced bug. [SHGetKnownFolderPath](https://learn.microsoft.com/en-gb/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath) guarantees a NUL-terminated allocation. The existing loop stops at that NUL and imposes a 32,768-unit cap. Replacing it with `lstrlenW`/`wcslen` would still rely on that allocation contract; a caller-supplied maximum cannot establish the allocation size either. No production change. |
| ANSI versus Unicode network block | Incorrect premise, already correct since `9e48ed5` / #245. The field at byte 8 **is NetNameOffset**. [MS-SHLLINK 2.3.2](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/23bb5877-e3dd-4799-9f50-79f05f938537) requires the Unicode offsets and Unicode share when it exceeds `0x14`. An ANSI-only layout at 21..27 is malformed. New fixtures pin ANSI at 20, Unicode at 28, and refusal of 21..27. |
| `pins()` call order | New scoped source-order test requires `volume(drive, None)` before `open_readonly_directory`. Deliberately moving the volume check after the open fails that test. This is structural proof, not instrumentation of OS calls. |
| Fuzz seed gaps | Both deterministic and property tests now share nine valid seeds: empty, local LinkInfo, Unicode name, IDList, environment, known-folder, tracker, ANSI network and Unicode network. Existing arbitrary-byte cases remain; their early header rejection is intentional complementary coverage. |
| Cold mutation timeout | Raised 60 to 300 seconds. An AST assertion requiring at least 300 failed before and passes after. |
| Listing-open zero attributes | Already fixed by `322e53b` / #257. `shortcut_opens_ignore_case_and_listing_opens_stay_exact` explicitly matches the listing helper's zero-attribute call, the shortcut helper's `OBJ_CASE_INSENSITIVE`, and assignment of those attributes to the native object. Existing mutation case replaces the zero with `OBJ_CASE_INSENSITIVE`. No windows-crate edit. |
| `subst` over a junction | Remains open. Source inspection still shows drive-type admission followed by a root-handle check; aliases can hide filesystem ancestors from the component walk. No per-session drive mapping was created, and no runtime claim about this case is made. Needs a dedicated native alias investigation and an explicit alias-admission rule before changing that behavior. |

## Changed

Only shell tests, shared synthetic fixture builders and the mutation runner changed.
Production parser and native behavior are unchanged. No dependencies added.
`HUMAN_TODO.md` is unchanged; no owner answer is inferred.

## Verified

Commands were executed with the absolute manifest path shown below. Baseline:
`cargo test --manifest-path C:/lw/wt/shell-260/Cargo.toml -p loomward-shell`:
31 passed (4 Windows unit, 4 property, 23 integration), 0 failed/ignored.

Regression-first: `cargo test --manifest-path C:/lw/wt/shell-260/Cargo.toml -p loomward-shell --test shell seed_corpus_reaches_all_optional_parser_branches -- --exact`
failed with the old three-seed corpus (0 passed, 1 failed), then passed with the expanded corpus.

Six new compiled mutations were run independently using
`cargo test --manifest-path C:/lw/wt/shell-260/Cargo.toml -p loomward-shell --test shell <test> -- --exact`:
dropping each of IDList/environment/known-folder/tracker, changing the network threshold
from `> 20` to `> 28`, and moving the volume check after the root open. All six reached
their assertions and failed (6/6 killed); original production bytes were restored in
`finally`. These cases are retained in `tests/mutate.py`.

| Final command | Result |
| --- | --- |
| `cargo test --manifest-path C:/lw/wt/shell-260/Cargo.toml -p loomward-shell` | PASS: 34 (4 Windows unit, 4 property, 26 integration), 0 failed/ignored; 0 doctests |
| `cargo clippy --manifest-path C:/lw/wt/shell-260/Cargo.toml -p loomward-shell --all-targets -- -D warnings` | PASS |
| `cargo fmt --manifest-path C:/lw/wt/shell-260/Cargo.toml --all --check` | PASS |
| `git -C C:/lw/wt/shell-260 diff --check` | PASS |

New files also receive `git diff --no-index --check` against `NUL`; exit 1 means
added content, with no whitespace diagnostics. Shared-target/cache lock waits were
observed and allowed to finish. No failing service timing tests were encountered.

## NOT verified / residual risk

The whole workspace, UI, hosted CI, full existing mutation corpus and non-Windows
execution were not run in this bounded shell lane. The driver owns workspace checks
and publication. The 300-second timeout is not a measured cold-build guarantee.
An OS allocation violating its documented NUL contract is not simulated.
The `subst` item above remains unresolved; do not close #260 as fully completed.
