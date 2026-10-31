# Selected collection curation

Mission 57, based on main `5ec82d2e9f6ab08462b270b33a9e619b1a4875a2`.

Collections now offers explicit selection of up to 500 durable member IDs across loaded pages, a clear
selection action, and one atomic removal. The existing single-member action uses the same path.
`remove_members` already supports unavailable objects; no production service, protocol, database,
provider or file-effect capability changed. Selection never means all unloaded members.

The action captures IDs, collection, displayed state revision and client/session context. A changed
revision refuses the set. Full refresh, stale-cursor restart, collection change and session invalidation
clear selection. Old reads, writes, errors, finally blocks and focus work cannot update a newer context.
A deadline or lost reply can follow a durable write: the view retains an uncertainty message, requires a
reread and never automatically replays the mutation. Successful batch text describes the submitted
selection, rather than claiming that every ID was newly removed. Label views remain read-only.

## Local proving checks

The first source-bound behavior test failed against the base because the existing action had no
selected-set submission. The raw red output is retained by the mission coordinator outside Git.
At the implementation checkpoint, 109 focused tests passed in four files; Svelte diagnostics reported
zero errors and warnings, Rust formatting and Python syntax passed, and `git diff --check` was clean.

Windows focused verification uses one worker:

```text
npm.cmd --prefix app test -- --maxWorkers=1 tests/unit/collection-curation.test.ts tests/unit/collections.test.ts tests/unit/collection-selection.test.ts tests/unit/view-races.test.ts
npm.cmd --prefix app run check
cargo fmt --all -- --check
py -3 -c "import ast,pathlib; ast.parse(pathlib.Path('scripts/test_app_collections.py').read_text(encoding='utf-8'))"
git diff --check
```

The handler tests execute functions extracted from the actual view, with controlled deferred replies.
They cover exact frozen payloads; single removal; 500/501 and empty bounds; unknown revisions and label
views; cross-page selection; stale-page retry limits; restart/collection/client invalidation; old reads,
errors and finally blocks; uncertain writes; failed post-write refresh; and late focus. They do not
substitute for Svelte binding or rendered-browser evidence. The mock contract test checks atomic stale
refusal, durable removal of revoked members, another collection and unchanged grants.

## Hosted proof required before merge

No substantial local allocation is inherited. Existing hosted CI is the proof route for the full app
suite/check/build, Python gates, native workspace tests/clippy/format, desktop build/tests and these
browser journeys. Local focused checks alone do not qualify this feature for merge.

- `crates/loomward-service/src/collections_tests.rs`,
  `collections_selected_batch_is_atomic_scoped_and_durable`: mixed resolved/unresolved removal, stale
  refusal without revision change, another collection unchanged, state-store reopen, unchanged grants
  and disposable fixture bytes/mtime. Run with `cargo test -p loomward-service collections_ -- --test-threads=1`
  or the hosted full workspace gate.
- `scripts/test_app_collections.py`, already called by `scripts/test_app.py`: mock selected add/remove,
  keyboard checkboxes, clear selection and restored focus.
- `py -3 scripts/test_app.py --live-serve`: 52 invented catalogue identities exercise the production app
  over real loopback HTTP; select across two pages including an unresolved member, force a stale revision,
  remove only the selected set, and keep another collection/grants unchanged. A routed response forwards
  the real mutation to commit before replacing its success with `deadline_exceeded`; the UI must retain
  uncertainty and reread without another write. A label-view fixture offers no removal controls.
- Desktop and narrow layout captures use the existing hosted artifact glob:
  `.loomward/search-root-collection-curation-{1360,390}.png`. These images show Collections; the prefix
  reuses the existing retention rule without changing a workflow.

The final PR and coordinator seal carry exact source identities, hosted run IDs, raw logs, artifact hashes,
review verdict and merge receipt. Test code and this planned gate list are not claims that hosted jobs ran.

## Boundaries

Only invented synthetic fixtures are used. No owner file, private path, model runtime, provider, device,
root grant or copy/archive/recycle effect is activated. Automated keyboard, layout and persistence checks
do not prove physical assistive-tech or device acceptance; HUMAN_TODO retains those separate decisions.
Parked destination-reserve and review-deferral work and completed seals remain unchanged.
