# Applications and desktop contexts: engine to app

Windows, 2026-10-11; branch `feat/apps-desktop-wiring`, uncommitted changes over
`e9ed2b5511eee6737ff99643580be5ff61b5d235`. Commands ran from this worktree's root.
No commit, push, release or external publication was performed.

## Changed

1. **Contract:** `apps.list` and `desktop.contexts`, available on HTTP and Tauri,
   `effects: none`, no events. Both take `MetadataPageRequest` (`limit` 1–200,
   required nullable `cursor`). DTOs, schema, examples, normative semantics, the
   Rust type registry and generated TypeScript changed together. The closed list
   now contains 49 commands. No command takes a path or an effect operand.
2. **Service:** new `apps.rs`, `desktop.rs` and `inventory.rs` modules; one dispatch
   line in `handlers.rs`. Dataset-bound injected providers keep synthetic sessions
   on fixtures and personal sessions on native metadata. Reads share the existing
   eight-permit pool and 2,000 ms default deadline. A timed-out native call retains
   its permit until it finishes. Pages retain a single immutable observation;
   cursors bind command/session and do not re-enumerate. There are 16 cursor slots,
   a 4 MiB encoded-snapshot bound, 6,144 application-entry bound and 400 synthetic
   shortcut-entry bound. Foreign/evicted cursors refuse with `cursor_expired`.
3. **Application evidence:** scope, literal text and expandability, registry-view
   provenance and provider coverage survive projection. Added the registry's
   `EstimatedSize` DWORD (KiB × 1024) as an estimate, never a filesystem measurement.
   Missing/malformed estimates remain null with a reason. Packaged and portable
   coverage remains explicitly unavailable; cross-view entries are not deduplicated.
4. **Desktop evidence:** five configured known folders retain ownership, current
   and default display paths, truncation, nullable redirection and unavailable
   provider reasons. Synthetic fixtures contain four shortcut entries with missing,
   volume-unavailable, network-unknown and unknown health. Arguments remain data.
5. **App:** Applications and Desktop views are discovered by the existing rail.
   They reuse tokens, byte/time formatting, escaped names, heading focus and the
   persistent announcement helper. Tables have captions, scoped headings and
   keyboard scrolling. Refresh clears old data; engine refusal shows an error;
   no uninstall, launch, repair, shortcut edit or execute control exists. The mock
   transport uses the same synthetic contract fixtures with bounded cursor paging.

## Personal shortcut enumeration: explicitly unavailable

Personal `desktop.contexts` returns real configured known-folder metadata but no
shortcut content or target reads. Both desktop providers report
`desktop_enumeration_unavailable`: the existing guarded shortcut reader accepts an
explicit file path and exposes no bounded pinned-directory listing API. Falling
back to pathname enumeration would weaken containment and no-follow guarantees.

A proposed pinned-handle listing extension failed three Windows access checks and
was removed under the finite attempt budget. Its code and temporary test are not
in this diff. The next prerequisite is a separately proved shell API that lists a
pinned desktop directory and reads capped children relative to that same handle,
refusing elevation, reparse/recall paths, network and removable/optical media.
Then the personal provider can replace the named unavailable result. No unsafe
fallback or manufactured synthetic personal result was added.

Elevated personal sessions refuse both commands with `personal_metadata_elevated`;
unsupported platforms refuse with `personal_metadata_unsupported`.

## Verified

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | PASS, exit 0 |
| `cargo test --workspace` (also run by `verify.py`) | PASS: 719 tests, zero failures |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `py -3 scripts/verify.py` | PASS: 224 Python tests, one existing symlink-privilege skip; both JS syntax gates, nine boundary assertions and 80 parity fixtures; Rust fmt/test/lint green |
| `cargo test -p loomward-service inventory::tests` | PASS: six new tests; service suite totals 83 tests |
| Protocol contract suites | PASS: schema/DTO/example/envelope round trips, bounds, closed command table and type registry |
| `npm.cmd --prefix app run check` | PASS: zero errors/warnings; viz guard green |
| `npm.cmd --prefix app run test` | PASS: 309 tests in 29 files |
| `npm.cmd --prefix app run build` | PASS, exit 0 |
| `py -3 scripts/test_app.py` | PASS: existing checks plus both new views |
| `py -3 scripts/test_app.py --a11y` | PASS, exit 0, including restored health-cell assertions |
| `py -3 scripts/test_app.py --live-serve` | PASS, exit 0 after restoring mutations |
| `git diff --check` | PASS, exit 0; new files also checked for whitespace |

The new Chromium helper runs in the mock, accessibility and live-service legs.
It checks both themes at 1440 and 390 CSS pixels, heading focus, keyboard table
scrolling, repeated refresh announcements, honest unknown sizes/provider states,
shortcut health cells, literal hostile names, absence of effect controls and
horizontal reflow. The live synthetic session traverses the real HTTP adapter,
service and injected fixtures. An intercepted refusal removes earlier application
rows; recovery returns the real synthetic service result. Existing harness checks
also establish loopback-only requests, no browser dialog and no console/page errors.
Eight synthetic screenshots were captured; desktop and narrow captures were
inspected. The Impeccable detector returned an empty findings list.

The normal-host personal service test checks only response shape, five configured
folders and five application-provider reports, with at most 200 rows. It records
no host names, paths, registry values or real inventory counts. On an elevated or
unsupported host it instead asserts both commands' refusal. Personal shortcut
reads do not occur on either branch.

## Regression and mutation results

Eleven of eleven mutations were detected. Seven Rust mutations compiled and then
failed assertions; no compilation failure was counted. Sources were restored
byte-for-byte in `finally`, and the restored suites/builds were rerun.

| New test | Deliberate regression detected |
|---|---|
| Python `read_only_pages_and_unknowns` | Remove `apps.list` from the contract |
| Apps `estimated_size_is_registry_evidence_not_disk_measurement` | Convert KiB using 1023 instead of 1024; the initial compiling stub also failed by returning Missing |
| Service `injected_synthetic_pages_bind_session_command_and_snapshot` | Resume a cursor at offset zero |
| Service `wrong_dataset_and_timed_out_providers_never_publish_fallback` | Disable dataset-class refusal |
| Service `native_personal_contexts_refuse_shortcut_reach_and_effects` | Falsely claim personal shortcut coverage is available |
| Service `projections_keep_registry_evidence_and_folder_unknowns` | Corrupt the reported estimate by one byte |
| Service `metadata_snapshots_and_cursors_have_finite_budgets` | Raise the snapshot limit so oversized output bypasses admission |
| Service `metadata_modules_expose_no_effect_or_unbounded_filesystem_api` | Add an inert forbidden execution-API source marker |
| Vitest application page test | Reset resumed mock cursors to offset zero |
| Vitest desktop health test | Change missing health to present |
| Chromium `run_inventory` | Render every shortcut health as present |

Both Vitest tests initially failed because the mock commands were absent. The
Python test initially failed because the command was absent. The browser mutation
failed its health assertion; a Windows log-decoding character interrupted the
runner's receipt step, which was reconciled from the assertion log. This was a
receipt encoding problem, not an additional test pass or an undetected mutation.
The browser mutation was then repeated against the final, cell-specific health
assertions, and failed on the first missing-health cell. The original UI was rebuilt.

No-effects evidence combines the closed contract, source guards on the new modules
and read-only registry rights, existing apps/shell no-effect and media-refusal tests,
the injected-provider separation tests and actual synthetic HTTP/browser checks.
No shortcut target, icon handler, registry writer or removable-media reader was
added. No destructive API was executed by a mutation (the forbidden marker is a
comment).

## NOT verified / residual risk

Personal shortcut enumeration and native Tauri/WebView2 interaction are unverified.
Hosted elevated Windows CI, Linux execution, physical removable-media transitions,
real registry ACL/churn scenarios and human assistive-technology use were not run.
No new Linux target was installed. Synchronous OS metadata calls can outlive a
deadline; read admission bounds their number, not the behavior of a blocked driver.
Provider estimates and point-in-time registry observations do not establish actual
disk usage, unused applications, complete coverage or permission for effects.

Recommended commit: `feat: wire read-only application and desktop metadata views`.
