# Copy-and-verify C7 UI evidence

17 October 2026, Windows. Refs #415; docs/47 section 9, sections 1/6/7, lane C7.
Base: `654a1a37fa08d30a0b744909ec1b1f18893afd71`, branch `feat/415-c7-copy-ui`.
The Sol lane leaves this change uncommitted for the driver.

## Changed

Explorer supports selection across folders and Copy to; Atlas offers Copy to for a selected
file/folder/root. The discovered Copy & verify view browses catalogue folders by node ID,
requests the existing preview contract, shows counts, exact byte strings, destination parent
and the single proposed folder, metadata policy, and each refusal's code and explanation.

Approval and start are separate explicit actions. Approval binds the operation and preview
digest, expires, is consumed before start, and is invalidated when inputs or observations change.
Old preview/approval responses cannot restore invalidated consent. An unknown start outcome
retains the operation for status lookup instead of starting again. Status, cancel, abandon,
blocked resume confirmation and the final no-source-space-freed receipt use the existing commands.
Cancellation disappears at publishing; changed sources require abandonment and a new preview.

Personal HTTP approval is unavailable. The existing mock still refuses every `copy.*` command:
no runtime synthetic bypass or copy executor was added. Full browser traversal uses a test-only
scripted synthetic engine, labelled as such in `scripts/test_app_copy.py`.

## Verified

Commands run from this worktree, with `npm.cmd --prefix app` (absolute prefix in the session):

| Command | Observed result |
|---|---|
| `npm.cmd --prefix app ci` | 66 packages installed; audit reports zero vulnerabilities |
| `npm.cmd --prefix app run check` | 0 errors, 0 warnings; visualization source check passes |
| `npm.cmd --prefix app run test -- --maxWorkers=2` | 44 files, 454 tests pass, including 15 copy-state tests and the Atlas selection regression |
| `npm.cmd --prefix app run build` | Production build passes |
| `py -3 scripts/test_app_copy.py --screenshots .loomward/c7-shots` | 10 checks pass: actual mock gate, all reported refusals, digest consent, invalidation, keyboard start, polling focus, cancel/abandon, publish, receipt, escaped names, reflow, personal HTTP refusal, zero console/page errors |
| `py -3 scripts/test_app.py --a11y` | All 216 checks pass on the final UI |
| `py -3 scripts/test_app.py` | Not green: both attempts stop at existing Atlas P9 hover assertion, median 30 fps versus required 50; repeat layout/paint median 46.5 ms passes its 50 ms gate. Later legs are not reached. |
| `cargo fmt --all --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test --workspace --no-fail-fast` | 1,171 pass, 0 fail, 0 ignored; the supplied host-failure warning was not reproduced on this run |
| `git diff --check` | Pass |

Cargo uses `CARGO_BUILD_JOBS=4` and the lane's own target directory throughout.
The first unit run intentionally failed before the state module existed. Browser regression
testing also caught and fixed focus loss caused by disabling the focused status button during
polling. Background status reads now preserve the controls and their focus.

Desktop (1440 px) and narrow (390 px) screenshots were rendered and visually inspected.
Raw logs and screenshots remain gitignored under `.loomward/c7-*` for the driver.

## NOT verified

No real copy effect, native picker/confirmation, hardware durability, screen-reader speech,
hosted CI or release was exercised. The entire default browser suite is not verified because
of its Atlas performance failure; it was not weakened or skipped.

The C4 contract has no native destination picker, interrupted-operation list, free-space
before/after fields, byte progress/rate, or detailed cleanup/leftover receipt. The UI explicitly
labels those limits and missing measurements rather than supplying demo values. Current-operation
state survives view navigation only; reload cannot discover interrupted operations. Personal
approval still requires Rust-owned confirmation when its backend gate opens.

## Residual risk

This proves a gated UI and a scripted lifecycle, not safe file copying. The copy effect gate
remains closed. No contract, backend, safety invariant, checkpoint, or `HUMAN_TODO.md` was changed.
Owner decisions remain in [HUMAN_TODO.md](../../HUMAN_TODO.md), including q-32.
