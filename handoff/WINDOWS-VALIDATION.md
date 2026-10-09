# Windows validation pack

## Environment record

Record OS edition/build and architecture, Python/Rust/Node versions, filesystem type and volume identity for the disposable test scope, available toolchain, elevation state and WebView2 version when testing Tauri. Do not publish usernames, machine names, serial numbers, private directory trees or process command lines. Hardware/tier measurements and configured limits are separate from device marketing names.

The basic Python reference requires Python 3.11+. Native Rust uses the MSVC toolchain on Windows; Tauri additionally requires its documented Microsoft C++ Build Tools/WebView2 prerequisites. Use the official references in docs/sources.json, not opaque install scripts. This package does not change ExecutionPolicy, register a service or install a driver.

## Stage A: Python reference, standard user

Create a disposable folder containing ordinary files, empty directories, Unicode and long names, equal-size distinct contents, equal contents, a project with a .git directory, a sensitive-named dummy file and inaccessible fixtures where supported. Never use real secrets as test fixtures. Launch the synthetic demo first, then an observed session selecting only that folder. Keep application state outside it.

Run the full unit suite. Verify that observed mode never silently falls back to demo, raw imported snapshots do not enable actions, filters/feedback/export stay profile-scoped, and restart retains approved labels. Confirm maximum-entry and excluded/denied states are visible. Export contents only to a private local destination. Check all source-file hashes before and after scan, duplicate inspection and labelling.

With explicit consent and optional psutil, compare read-only process metrics against Windows tools. Check first-sample CPU unknown handling, CPU normalisation, process disappearance, permission denial and PID reuse. Do not interpret RSS totals as unique physical RAM or trim a process to lower a chart.

## Stage B: native compilation

Run root workspace formatting, tests and strict lint. Generate/review Cargo.lock. Fix source issues with regression tests and record the toolchain. Run `cargo run -p loomward-cli -- capabilities`, a tiny `scan` and the shared `plan` fixture after compilation. Match shared semantics rather than inventing native test evidence from Python results.

The root workspace excludes native/. Install/use the Tauri tooling only under the owner's environment policy. Compile that project separately, check its empty custom capabilities and synthetic-only page, and verify that no broadly scoped filesystem/shell/process plugin slipped in. No current test proves the Tauri scaffold builds.

## Stage C: native filesystem identity lab

Use LW-064 and LW-003 acceptance criteria. Exercise NTFS ordinary files, long paths, case variants, junctions/reparse points and ancestor substitution, hard links, alternate data streams, sparse/compressed files, ACLs, open/share-denied handles and renames during observation. Add cloud placeholders only through a configured disposable test provider and prove no hydration/content-read side effects from the metadata pass. Simulate offline/removable destination loss with disposable data.

Do not turn on new privileges to make a failing test disappear. Specify the needed privilege/capability and treat unsupported environments as explicit partial coverage. On FAT/exFAT/ReFS/network shares, test the documented fallback or report unsupported identity semantics rather than using an NTFS assumption.

## Stage D: browser and packaging

Use actual localhost navigation with the printed session fragment in Edge/Chrome and later real WebView2. The original harness used a Python fetch bridge because direct navigation was blocked. Repeat origin/host/session checks with real browser networking. Check refresh, skip link, keyboard focus, screen reader labels, high contrast, 125/150/200% DPI, long translations, narrow width, loading/error/offline and 50k/200k record boundaries.

Future million-object tests must use paged/virtualised native queries. A synthetic 18-file preview is not a performance claim. Reconcile keyboard/visual tests with WCAG targets before saying accessible rather than simply keyboard-usable.

## Stage E: live teacher, separately authorised

Select an existing local model and verify its exact structured-output support. Inspect the offline request first. Use a synthetic fixture for the first call. Confirm token/timeout/response bounds, malformed/abstaining output behavior, TTL compatibility and no redirects. Record model identity/version, inference settings and result without private metadata. No actual model was tested in the initial pass. The current adapter does not supply bearer authentication; add an explicitly reviewed bounded configuration if the chosen local server requires it.

## Exit criteria

Reference behavior passes on the recorded Windows machine, native sources compile and pass relevant tests, unsupported cases remain visible, no unapproved file/process changes occur, and evidence is attached to the actual commit. This is permission to continue native observation work, not a release approval or grant to move files.
