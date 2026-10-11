# Duplicate inspection wiring (#281)

Local proof recorded 2026-10-10 on Windows, targeting `x86_64-pc-windows-msvc`. Software:
Python 3.14.3, rustc 1.97.1, Node 24.13.1, npm 11.8.0. All inventories, names and content
in these fixtures are synthetic. No personal root was granted or read. No commit or push was made.

## Changed

1. `crates/loomward-catalog/src/duplicates.rs`: one-root, exact-logical-size candidate selection,
   default floor 1, no empty files, groups with at least two visible names. The scan's volume,
   file ID, attributes and creation/write/change observations accompany root-relative names.
   Existing visibility/grant rules and explicit excluded-ancestor shielding apply. Candidate
   cardinality and aggregate JSON-encoded path bytes (64 KiB) are bounded, with `truncated`.
   A cut leaving a one-name size group removes that group without claiming it was absent.
2. `crates/loomward-engine/src/duplicates.rs` and `src/jobs/mod.rs`: a reserved worker runs the
   existing inspection reader under the job registry. Metadata hydration uses the same pinned
   root-relative reader path; scan observations must match before content is read. Job `seq` and
   `rev`, retained results, ordinary cancellation and root revocation use existing seams.
   Reports publish atomically with the terminal revision. Revocation discards pending evidence;
   ordinary cancellation can retain partial evidence with `stop=cancelled`.
3. `crates/loomward-service/src/duplicates.rs`: `duplicates.inspect` and `duplicates.cancel`;
   results read through `jobs.get`/`jobs.list` and terminal `job.state`. Existing session request-ID
   idempotency, mutation permit/FIFO and call deadlines apply. Dispatch changes are two lines in
   `handlers.rs`. Commands, Rust DTOs, schema, semantics, command examples, terminal-result example
   and generated TypeScript agree. Both service and engine refuse personal inspection before I/O
   with `content_read_gated`; personal scan gating remains unchanged.
4. `app/src/views/duplicates/View.svelte`: a rail view, because root selection, a cancellable job
   and its retained evidence form a separate workflow from Explorer's paginated tree. Existing
   design tokens, named controls, row headers, a live status region and terminal-result focus are
   reused. Verified sizes, one-object link sets with no space to reclaim, skipped reasons, bytes,
   partial/truncated/stop are visible. Equal content is an observation, not deletion advice.
   `duplicates.svelte.ts` orders snapshots and events by `seq`/`rev`; the shared mock job registry
   supplies an explicitly invented fixture. Store Vitest and Playwright cover the surface.
5. This receipt contains aggregate proof only. Raw logs and synthetic screenshots remain in
   gitignored `.loomward/dup281/`; they are not publication artifacts.

## Regression and mutation proof

Every added behavioral test was exercised with its protection removed and observed to fail by
an assertion; compilation/setup errors were not counted. Mutated files were restored byte for
byte before proving the implementation. Fifteen runs were killed, including a repeat of registry
cancellation after the terminal-publication fix:

| Removed protection | Regression that failed |
|---|---|
| Candidate singleton pruning; encoded-path budget | `duplicates_select_exact_sizes_visible_names_and_scan_identity_with_a_cap` |
| Native hard-link partition | `duplicates_native_service_job_reads_synthetic_content_and_never_counts_links_as_duplicates` |
| Personal service refusal | `duplicates_personal_refusal_precedes_any_root_or_content_access_and_replays` |
| Personal engine admission | `duplicate_content_is_gated_for_personal_grants` (extended to submission) |
| Registry cancel flag; reader job-cancel forwarding; atomic terminal publication | `duplicate_job_cancel_and_root_revocation_reach_the_worker_and_terminal_state` |
| Pinned metadata hydration (unsafe path observation substituted) | `native_candidate_hydration_is_root_relative_and_never_follows_a_junction` |
| Contract group uniqueness | `test_duplicate_examples_validate_requests_and_terminal_evidence` |
| Store ordering; gated state; cancel dispatch | Three `duplicates-store.test.ts` cases |
| Verified group rendering | Playwright Duplicates step in `scripts/test_app.py` |

The native service fixture also pins exact content grouping against same-size unequal content,
link classification, idempotent submission replay, increasing job counters, zero-byte budget,
and rejection of a file changed after scanning. The engine test covers ordinary cancellation and
revocation both before reading and after reading but before terminal publication. Native hydration
rejects traversal and a junction. The browser step checks sizes, evidence, skipped reasons, result
focus, cancellation and 375-pixel reflow. Desktop and mobile screenshots were inspected.

## Verified

Every command below exited 0. Overlapping Rust runs are not additive test counts.

| Command | Final output |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | 636 passed, 0 failed, 1 existing ignored test |
| `cargo test -p loomward-windows -p loomward-engine --all-features` | 230 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS, no warnings |
| `py -3 scripts/verify.py` | PASS: 223 Python tests (1 existing skip), two JS syntax checks, 9 JS boundary assertions, 80 cross-language fixtures; workspace 636 passed/1 ignored; Rust format/lint PASS |
| `npm.cmd --prefix app ci` | PASS, 66 packages installed, 0 vulnerabilities reported |
| `npm.cmd --prefix app run check` | 356 files, 0 errors, 0 warnings; visualization check PASS |
| `npm.cmd --prefix app run test` | 27 files, 258 tests passed (including 3 new store tests) |
| `npm.cmd --prefix app run build` | PASS, 175 modules transformed |
| `py -3 scripts/test_app.py --screenshots .loomward/dup281/screenshots` | ALL PASS, 175 printed PASS checks |
| `py -3 scripts/test_app.py --a11y` | ALL PASS, 132 printed PASS checks |
| `py -3 scripts/test_app.py --live-serve` | ALL PASS, 30 printed PASS checks; 4 diagnostic unit tests also passed |
| `py -3 .loomward/dup281/native_http.py` (disposable smoke test) | PASS: actual native scan, inspection, HTTP transport and browser; 5 fixture files, 1 verified group, 1 link set, 28 bytes read, partial=true, stop=null, terminal focus, no page errors |
| `git diff --check` | PASS; new-file whitespace checked separately |

The Python skip is the existing symlink-privilege case. The Rust ignore is the existing
`verbatim_device_namespace_state_paths_are_refused` test (tracked by #246). Neither was added
by this work. Native duplicate-service, cancellation/revocation and junction hydration mutations
reached their assertions, demonstrating that the native fixture legs actually executed.
The accessibility leg checks the existing app suite; Duplicates-specific row-header roles,
result focus and narrow-screen geometry are pinned by the standard browser step.

Initial full-suite runs exposed stale contract fixture/count expectations (the command table now
has 47 commands) and Clippy's item-order rule for the new metadata helper. Those were corrected;
no failing check was weakened or skipped.

Browser regression rendering uses the synthetic mock; real-service refusal/gating is covered by
the live-server leg. A separate disposable smoke test also verified successful native inspection
through the real HTTP server and production browser app. It registered an empty synthetic lab root
with the existing lab registry, created five tiny fixture files (including two hard-link names),
scanned that root, started inspection from the browser and checked the result and focus. The helper
and script remain gitignored; the temporary server, fixture and state were cleaned up.

## NOT verified

Hosted CI, non-Windows execution, native Tauri/WebView2 operation, and actual NVDA/Narrator speech.
Personal content reads are intentionally unavailable. No deletion, movement, rename, link creation
or reclaim estimate is implemented; those were declined as outside this issue's scope.

## Residual risk

Equality describes verified default streams at observation time, not a filesystem snapshot or
permission to act. Cancellation is non-joining; synchronous filesystem I/O must return before the
worker drains. Partial, skipped and truncated evidence remain visible. Link sets are one object,
with no claimed space to reclaim.

Recommended commit message: `Wire read-only synthetic duplicate inspection end to end`.
