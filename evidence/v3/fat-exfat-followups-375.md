# FAT/exFAT follow-ups (#375)

Baseline: `00bb610` on a Windows 11 laptop, 2026-10-17; verification continued
on 2026-10-18. The freshly fetched
`origin/main` at `f39b75d` changes the developer-clutter surface, not these grant,
enumeration or dialog paths. All fixtures here are disposable and synthetic.

## Issue disposition

- Active re-grants skip a fresh filesystem observation; a legacy NULL therefore
  survives. Refresh the observation on the specific existing grant without
  replacing it. A conditional `RefreshRoot` writer command verifies that grant's
  active state and native binding in the state transaction; it cannot fall back
  to creating a new grant if another writer revokes it after the service lookup.
  `RegisterRoot` retains its original fresh-registration behavior.
- Restarting with the same unverified root asks for the CLI confirmation flag
  again. Reuse the stored unverified permission only for the same active native
  root identity; a new root still needs explicit confirmation.
- A catalogue rebuild loses the filesystem. Store it with the precious grant,
  recover known legacy values from an intact catalogue during migration, and
  reconstruct the volume from that stored observation. Missing observations
  remain NULL; never infer NTFS from the old grant's default identity mode.
- Folder confirmation is already fixed by `282d9a9` (PR #379): the picker names
  the folder and defaults to Cancel. The separate identity warning still needs
  both the selected folder and observed filesystem, passed in Rust only.
- Destination placement policy remains open: #375 explicitly asks for a product
  decision. This lane does not choose which unverified destinations a simulation
  may recommend. Existing source-row refusal stays in force. The current volume
  projection has no filesystem-specific destination filter; personal simulations
  still refuse missing source-volume physics (`personal_placement_physics_unobserved`).
  That existing guard is not a FAT/exFAT destination policy. HUMAN_TODO q-12
  answers metadata-only listing and proposal-backed rows, without separately
  selecting simulation destination eligibility.
- Strengthen the containment test with missing-ID Find: unverified mode must
  reach the failing containment check, while NTFS must refuse the absent IDs
  first. The ordinary containment failure must still refuse before emitting any
  rows. Assert non-elevation so this regression cannot silently return early.
  The standalone missing-ID Find control already existed in `68cb029` (PR
  #372), verified by blame on `find_binds_unverified_pins_by_path_but_never_on_ntfs`;
  its functionality is retained rather than implemented again. Other legacy
  native tests' elevation returns are outside this targeted test change.
- Real FAT/exFAT hardware remains unverified. Injected filesystem names on NTFS
  do not establish whether a real FAT grant has a usable weak root ID.

`HUMAN_TODO.md` q-12 records the owner's metadata-only, identity-unverified
listing decision and explicitly leaves the real-volume leg unverified. No
human decision is inferred or marked complete by these fixtures.

The state and catalogue version advance to v9 under the existing backed-up
migration path. The existing newer-database guard means older v8 builds will
refuse an upgraded store; this lane adds no downgrade migration.

## Verification

The lane leaves changes uncommitted for the driver. Cargo uses the exported
shared target pool, `CARGO_BUILD_JOBS=4`; the scoped test runs additionally use
`RUST_TEST_THREADS=1`. No workspace-wide suite is run in this lane.

Before each corresponding fix:

| Command | Result |
| --- | --- |
| `cargo test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-catalog --test open_guards grant_filesystem -- --nocapture` | 0 passed, 2 failed: reconstruction returned NULL instead of NTFS; legacy state lacked the filesystem column |
| `cargo test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-service regrant_backfills -- --nocapture` | 0 passed, 1 failed: re-grant left the derived filesystem NULL |
| `C:/lw/target/pool-b/debug/deps/loomward_service-0cd028a8b3f776ca.exe --exact tests::stored_unverified_grant_needs_no_second_cli_confirmation_after_restart --nocapture` | 0 passed, 1 failed: the retained baseline binary returned IdentityUnverified after reopening the service |
| `cargo test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-catalog --test open_guards regrant_filesystem -- --nocapture` | 0 passed, 2 failed: registration created another grant and accepted an identity-mode change. Run after the persistence fix, before the registration fix |
| `cargo test --manifest-path C:/lw/wt/fat-375/native/Cargo.toml identity_unverified_confirmation -- --nocapture` | 0 passed, 1 failed: the native prompt omitted synthetic-folder. The API accepted context but the pre-fix display still ignored it |
| `cargo --config C:/lw/wt/fat-375/.loomward/fat375/before-root.toml test --manifest-path C:/lw/wt/fat-375/.loomward/fat375/race-before-gv97m_01/Cargo.toml -p loomward-service revocation_between_regrant_lookup_and_write -- --nocapture` | 0 passed, 1 failed: a revoked grant supplied a saved confirmation. The immutable pre-correction snapshot contains the race test and hook |

The redundant queued Cargo rebuild for the restart red test was stopped only
after the same test had failed from the retained baseline binary. No other
lane's processes or target files were changed.

Completed static checks:

- `cargo fmt --manifest-path C:/lw/wt/fat-375/Cargo.toml --all --check`: exit 0.
- `git -C C:/lw/wt/fat-375 diff --check`: exit 0.
- Native-build prerequisite: `npm.cmd --prefix C:/lw/wt/fat-375/app ci --ignore-scripts --no-audit --no-fund`
  installed the 66 locked packages; `npm.cmd --prefix C:/lw/wt/fat-375/app run build`
  passed (226 modules). App sources and its lockfile are unchanged. This is build
  preparation, not a claim of app interaction or browser verification.

Completed green checks:

These precede the additional revoke/refresh concurrency correction; final-source
qualification below supersedes them where that correction changes the seam.

- `cargo test --manifest-path C:/lw/wt/fat-375/native/Cargo.toml`: 59 passed,
  0 failed, 1 ignored (18 library, 35 desktop integration, 2 install-guard,
  4 update-qualification tests). The named identity-warning regression passed.
- `cargo clippy --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-catalog -p loomward-protocol -p loomward-service -p loomward-windows --all-targets -- -D warnings`:
  exit 0, no warnings.
- Both root and native `cargo fmt --check` checks and `git diff --check`: exit 0
  after the final code edits.

An extra combined test invocation (`cargo test --manifest-path
C:/lw/wt/fat-375/Cargo.toml -p loomward-catalog -p loomward-protocol -p
loomward-service -p loomward-windows --no-fail-fast`) was interrupted, not
counted as passing. Combining the selected packages unified the service's
`test-hooks` dev dependency into the catalogue package, enabling its optional
`crash_sweep` target. Before interruption, the 72 catalogue unit tests and
7 `catalog_findings` tests passed, with no reported failure. Only this lane's
verified Cargo process and its descendants were stopped. The task's required
`cargo test -p <crate>` suites are instead run separately, without changing test
code, adding skips, or changing default features. The optional extended sweep
remains unverified; its partial output is in the ignored local logs.

Final-source qualification (same shared target, temporary profiles described
below):

- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-root.toml test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-catalog --no-fail-fast`:
  270 passed, 0 failed (72 unit and 198 integration tests); includes all six new
  catalogue tests and the previously timed-out checkpoint unit test.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-root.toml test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-protocol --no-fail-fast`:
  54 passed, 0 failed.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-root.toml test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-windows --no-fail-fast`:
  122 passed, 0 failed; includes the strengthened containment regression on this
  non-elevated Windows host.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-root.toml test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-service --no-fail-fast`:
  final rerun 320 passed, 0 failed, 1 existing ignored (319 unit passes and one
  integration pass); includes all new grant, restart, race and picker tests.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-root.toml clippy --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-catalog -p loomward-protocol -p loomward-service -p loomward-windows --all-targets -- -D warnings`:
  exit 0, no warnings, after the diagnostics fixture correction.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-root.toml clippy --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-windows --all-features --all-targets -- -D warnings`:
  exit 0, no warnings.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-root.toml test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-windows --all-features --no-fail-fast`:
  200 passed, 0 failed, 1 existing ignored (192 unit passes, 8 decoder tests).
  Includes the existing 200,000-entry staging-budget fixture.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-root.toml test --manifest-path C:/lw/wt/fat-375/Cargo.toml -p loomward-service revocation_between_regrant_lookup_and_write -- --nocapture`:
  1 passed, 0 failed; the unchanged race test now refuses the revoked grant.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-native.toml test --manifest-path C:/lw/wt/fat-375/native/Cargo.toml`:
  59 passed, 0 failed, 1 existing ignored; includes the warning-context regression.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-native.toml build --manifest-path C:/lw/wt/fat-375/native/Cargo.toml`:
  exit 0. `C:/lw/target/pool-b/debug/loomward-desktop.exe --self-test`:
  14 PASS checks and `self-test passed`, exit 0.
- `cargo --config C:/lw/wt/fat-375/.loomward/fat375/final-native.toml clippy --manifest-path C:/lw/wt/fat-375/native/Cargo.toml --all-targets -- -D warnings`:
  exit 0, no warnings.

The immutable race baseline comprises 1,318 tracked working-copy files with
content digest `a9fa9406872b97fcf882931227db6655948d7dfd221a59803d121b02e7eb139e`.
One earlier baseline attempt and one final-source attempt failed compilation:
the compiler reported the old `NativeDialogs` signature despite the new
signature being present in both source trees. A stale shared artifact was the
working diagnosis; those are setup failures, not regression-test failures. No
shared target file was removed. Qualification uses temporary Cargo
package profiles for each local `loomward-*` package, for both `dev` and `test`:
baseline `debug=0, codegen-units=37`; final root/native `debug=1,
codegen-units=41`. This gives the local artifacts distinct cache identities
while retaining the mandated shared target, default features, optimization and
assertion settings. The ignored local configs are `before-root.toml`,
`final-root.toml` and `final-native.toml` under `.loomward/fat375/`.
The timestamp-refreshed, unqualified baseline retry also reached the same
runtime failure (0 passed, 1 failed); its bytes matched the recorded digest.

An earlier default per-crate catalogue run failed its unchanged timing-sensitive
`writer::tests::end_run_checkpoint_does_not_wait_for_a_reader_and_restores_write_timeout`
test (71 unit tests passed, 1 failed; integration targets passed). Running that
exact test alone from the same executable passed: 1 passed, 0 failed in 0.69 s.
The full failed run is not counted as green. Command:
`C:/lw/target/pool-b/debug/deps/loomward_catalog-992d2b241dfcc9a7.exe --exact writer::tests::end_run_checkpoint_does_not_wait_for_a_reader_and_restores_write_timeout --nocapture --test-threads=1`.

The first final-source service run completed with 317 passed, 3 failed and
1 ignored (316 unit passes plus the integration pass). One failure was a stale
diagnostics expectation of schema v8; the two expected version values were
updated to v9. Its failure was observed before this fixture correction. The
other two tests passed unchanged, alone from the same executable:

- `C:/lw/target/pool-b/debug/deps/loomward_service-2ea403057ea8d349.exe --exact tests::a_duplicates_job_started_past_its_deadline_reports_the_started_job --nocapture --test-threads=1`:
  1 passed, 0 failed, 1.71 s (full run returned Cancelled).
- `C:/lw/target/pool-b/debug/deps/loomward_service-2ea403057ea8d349.exe --exact tests::copy_tests::lab_copy::sources_from_two_granted_roots_are_refused --nocapture --test-threads=1`:
  1 passed, 0 failed, 2.18 s (full run failed to catch up with a stale refresh).

These are timing-sensitive failures in unchanged cases under shared-host load;
neither test nor timeout was weakened. The full failed run is retained in the
receipt separately from the isolated passes and subsequent full service rerun.

Final static checks after the only subsequent code change (the two diagnostics
version expectations): root `cargo fmt --manifest-path
C:/lw/wt/fat-375/Cargo.toml --all --check` and `git -C
C:/lw/wt/fat-375 diff --check` both exit 0. Native formatting also passed and its
source has not changed since. No failing check remains in the final scoped pass.

## Left open

- No real FAT/exFAT volume or weak root ID was exercised; no physical-media or
  real native dialog interaction claim is made. HUMAN_TODO q-12 and the hardware
  checklist remain unchanged.
- No destination-policy choice was made. That #375 item needs a product
  decision, and existing metadata-only/source-row refusals remain in force.
- Broader legacy tests that return early on elevation remain outside this lane;
  the changed native containment test asserts non-elevation and ran here.
- The optional catalogue crash sweep interrupted earlier remains unverified.
  The complete default catalogue suite subsequently passed. Workspace-wide
  qualification belongs to the driver and was not run in this lane.
- No commit, push, PR, hosted CI or release was created. Changes and this receipt
  are left in `fix/375-fat-exfat` for the driver. Schema v9 has no downgrade
  migration, so older v8 binaries refuse the upgraded stores.
