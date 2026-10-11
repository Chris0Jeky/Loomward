# Personal learning, local student only (HUMAN_TODO q-11)

Measured 2026-10-11 on Windows 11 (DESKTOP-IHKOOJS, shared host, non-elevated). Branch
`feat/personal-learning-q11` from `2944542`. Every model number below comes from synthetic fixture
names on disposable state directories: **synthetic smoke, not real-user accuracy**. No real
path, file name or inventory was scanned, labelled or committed.

## Owner decision applied

q-11 (2026-10-11): the local Rust student trains on the owner's human labels plus metadata
features (name, extension, two ancestor names, size) of granted roots, on this machine.
Suggestions stay advice with relative scores. The teacher stays synthetic-only; q-5's conditions
are unmet, so every teacher path refuses personal sessions. `learning_personal_gated` is retired.

## Changed

1. `crates/loomward-engine/src/learning/mod.rs`
   - The personal `learning_gate` is gone (feedback, replay, prior, validation, refit, queue,
     meaning, status). Status no longer substitutes an empty student for personal sessions;
     `gate` is null.
   - `learning_store` (line 188) refuses with `permission_denied` / `dataset_class_mismatch`
     unless the store's recorded `st.meta.dataset_class` equals the engine's (line 199). This sits
     behind the existing `Catalog::validate_stores` refusal at service open.
   - `Store::redact` (line 115) records in `st.learning_redacted` every human event whose
     object's bound catalogue row lies under a grant that is no longer `active`, rewrites its
     `features_json` to the empty snapshot `REDACTED` (line 43), deletes every `student_model` row
     (a model's state carries name tokens) and bumps `state_rev`. It is idempotent and runs at
     store open, at the start of each refit (line 421) and from `learning_redact_revoked`
     (line 207). The refit query (line 426) excludes redacted events.
2. `crates/loomward-service/src/lib.rs:460-478`: `teacher.preview`, `grants.create_disclosure`,
   `teacher.run` and `teacher.results` refuse a personal session with `capability_unavailable` /
   `detail.reason: teacher_personal_gated` once the envelope validates, ahead of the wave gate.
   The engine's own `teacher_run` refusal (`confinement_not_enforced`) stays.
3. `crates/loomward-service/src/handlers.rs:483,513`: revocation and `identity_changed` call the
   sweep after their durable commit. `learning.rs` and `meaning.rs` lose the personal gate and the
   personal empty-queue shortcut.
4. Contracts: `semantics.md` section 3 (personal learning, revocation, teacher refusal),
   section 11 and the meaning section; `commands.json` notes for `learning.status` and the four
   teacher commands; the `LearningStatus.gate` description in `view-service.schema.json`. No DTO
   shape changed; `scripts/gen_contracts_ts.py` regenerates `contracts.gen.ts` byte-identical.
5. `app/src/views/review/View.svelte`: personal sessions see "Your labels train a local student
   on this machine. The teacher never sees personal data.", a "Personal candidate" status line
   and "Advice only, never an approval." The gated note is now generic (`Learning is gated (…)`).
6. `scripts/test_app.py`: the personal scan leg now asserts an open Review surface and the four
   teacher refusals; the new `live_personal_learning_isolated` leg (line 504) plants a disposable
   personal-session fixture root (origin `owner_granted`, four synthetic `.bin` names, NTFS) in a
   fresh `--dataset personal --allow-personal` server and drives the real HTTP service and app.

## Guarantees and their tests

| Guarantee | Test |
|---|---|
| Personal sessions record and withdraw labels, read the queue, refit human-only, read meaning | `learning_personal_session_learns_locally_and_revocation_redacts`; live `personal learning:` checks (16) |
| Revocation redacts features, deletes models, withdraws from training | same Rust test; live "revocation deletes the candidate" and "withdraws every label" |
| A revoked root's nodes cannot be labelled | same tests (`not_found`) |
| Labels can always be withdrawn (#351) | same tests (retraction after revocation succeeds); existing `learning_retracts_without_current_node_access_and_beyond_history_budget` |
| Human labels only; teacher rows never train (invariant 3) | personal Rust test plants a teacher row on the labelled object: `training_count` 1, `teacher_labels` 1, `human_support.Archive` 0; existing synthetic `learning_refit_is_human_only_candidate_and_retraction_withdraws` |
| Personal and synthetic evidence never mix, both directions | `learning_stores_never_cross_dataset_classes`: reopening either class's store as the other refuses at service open, and an engine handed the wrong store refuses status and refit |
| Relative scores, never probabilities | `calibrated: false`, `autonomy_allowed: false` asserted in Rust and live; UI keeps "Relative scores, not probabilities" |
| Every teacher entry point refuses personal sessions | Rust test and both live personal legs: four commands, `teacher_personal_gated` |

`meaning_is_bounded_read_only_advice_with_explicit_unknowns` now expects a personal unknown node
to be `not_found` instead of the retired gate.

### Mutations (each restored afterwards; failures are behavioural assertions)

| Mutation | Result |
|---|---|
| Refit ignores `st.learning_redacted` | KILLED: "revocation withdraws", training_count 1 (Rust); live "withdraws every label" |
| Sweep does not rewrite `features_json` | KILLED: "revocation redacts every label's features" |
| Sweep keeps `student_model` rows | KILLED: model count 1 |
| No sweep on revocation | KILLED: features still present after `roots.revoke` |
| Teacher gate removed | KILLED: `teacher.preview` reason None (Rust); live "teacher.preview refuses" |
| Store dataset class unchecked | KILLED: wrong-class engine returned a status |
| Teacher rows unioned into training | KILLED: `human_support.Archive` 1 |
| Catalogue `get` no longer requires an active grant | SURVIVED: a second catalogue layer (`inspect` reads the grant's revisions) still refuses with `not_found`; the revoked-node guard is layered, not single |

## Verification

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` (default parallel) | FAIL: `meaning_work_budget_is_an_honest_prefix` hit its read deadline under load; the same test fails the same way on the unmodified base, and passes alone |
| `RUST_TEST_THREADS=1 cargo test --workspace` | PASS, 838 passed, 0 failed, 0 ignored |
| `RUST_TEST_THREADS=1 py -3 scripts/verify.py` | PASS: 227 Python tests (1 symlink-privilege skip), JS syntax, 9 JS boundary assertions, 80 cross-language fixtures, Rust fmt/tests/lint |
| `npm.cmd --prefix app run check` | PASS, 0 errors, 0 warnings |
| `npm.cmd --prefix app run test` | PASS, 331 tests in 32 files |
| `npm.cmd --prefix app run build` | PASS |
| `py -3 scripts/test_app.py` | PASS, 212 printed checks |
| `py -3 scripts/test_app.py --live-serve` | PASS, 98 printed checks; one earlier run timed out on an unrelated `page.goto` and one on a click under host load, both passing on rerun |
| `git diff --check origin/main..HEAD` | PASS |

## Finding: real scans never yield a durable reference

The live personal scan leg's catalogue shows `volume.filesystem` NULL after a complete scan:
`Inner::grant` registers every root with `filesystem: None`
(`crates/loomward-service/src/lib.rs:885`, `filesystem: None` in its `RootObservation`), and durable references and
the review queue require NTFS or ReFS. On a real scanned root the queue is therefore empty and
`feedback.record` refuses with `durable_reference_unavailable`, in both dataset classes. The
learning surfaces here are proven on SQL fixture roots, as L14's were. Opening q-11 for real
folders needs the grant path to record the volume's filesystem; that is outside this slice.

**Resolved after #372 (still within #368).** #372 made every grant record its volume's real filesystem. #368's
commit 9fa5159 then added a service test in which a personal session labels a file in a real granted NTFS folder.
That test is in `crates/loomward-service/src/tests.rs`.

## NOT verified

Learning on a real owner-picked folder through the desktop picker (the NTFS leg is a service test since 9fa5159); Tauri/WebView2 native UI;
hosted or elevated CI; real-user accuracy, calibration or promotion; manual screen-reader
testing; redaction of events whose reference awaits a pending rebind or whose catalogue instance
was rebuilt (the sweep maps through the reference's bound row; see residual risk).

## Residual risk

- The sweep maps events to roots through `object_ref.row_id` in the current catalogue instance.
  An event bound to a rebuilt catalogue instance, or to a row superseded by a pending rebind, is
  not redacted until a later sweep maps it; those events are already unresolved for training.
- Redaction deletes every stored model on any change, synthetic or personal; the next refit
  rebuilds from the remaining labels.
- `human_events` still counts redacted events; only `training_count` drops.
- Default parallel workspace tests stay sensitive to shared-host scheduling (pre-existing).
