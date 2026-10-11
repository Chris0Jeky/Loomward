# Desktop preview prerequisites (#373, 11 October 2026)

Measured on Windows 11 (DESKTOP-IHKOOJS), without elevation, on branch
`feat/desktop-preview-prereqs-373` (based on `main` after #371). These are the #371 review findings to
fix before the first unsigned preview pre-release (HUMAN_TODO q-17). Every run below used a scratch
`--state-dir` or a temporary `%LOCALAPPDATA%` stand-in; nothing opened the owner's state, and no
picker was accepted on a real folder.

## What changed

- **Picker start and confirmation** (`native/src/dialogs.rs`). The folder picker opens on the user's
  profile folder (`USERPROFILE`) with nothing selected, instead of the folder Windows remembers (how
  the #371 development incident granted a real folder). Why the profile rather than the Desktop: a
  stray Enter in a folder picker picks the folder being shown, and the whole profile is the one
  starting view the grant rules refuse anyway (`protected_profile_or_store`); the Desktop would be
  grantable. With no usable profile the picker does not open (fail closed; the service then reports
  `cancelled_by_user`, because `NativeDialogs::pick_folder` has no error channel). After a pick, a
  main-window-owned `MessageBoxW` whose default button is Cancel asks "Let Loomward read the names,
  sizes and dates in <folder>?" and says it never changes files and reads no contents unless asked
  for duplicates. Letters of any script are shown; controls, bidi overrides and zero-width
  characters are escaped as `\u{..}`. Declining returns `cancelled_by_user` and creates no grant;
  with no owner window the box is never shown and the pick declines.
- **FAT/exFAT.** #372 (identity-unverified volumes) was not merged when this landed, so there is no
  separate FAT/exFAT confirmation in the picker yet. When #372 lands, its refusal still applies in
  the service after this confirmation.
- **Single instance** (`native/src/launch.rs`). `launch::open` takes docs/41 section 3's exclusive
  `lock` file in the state directory before the stores open (`File::try_lock`, so `rust-version` is
  now 1.89) and keeps it inside the service object it returns, so the lock is released only after
  the service drops. It is an OS lock on an open handle: a killed instance leaves nothing stale. A
  second instance is refused with `busy: Loomward is already open on <dir>` and exits non-zero. It
  does not focus the first window: that needs the Tauri single-instance plugin (or window-search
  code), and the shell registers no plugins; the lock is the correctness guard. `loomward-serve`
  takes no lock.
- **Visible launch errors** (`native/src/main.rs`). Argument, launch and runtime failures print to
  stderr when there is one (a parent console, the debug build's own console, or a redirected
  handle). With none, as when an installed build starts from the Start menu, an error box titled
  "Loomward could not start" shows the reason before the process exits non-zero.
- **State outside the install directory** (`native/src/launch.rs`). The default state directory is
  `%LOCALAPPDATA%\Loomward Data\v3\<class>`, a sibling of the per-user install directory
  `%LOCALAPPDATA%\Loomward`. A default start moves an existing `%LOCALAPPDATA%\Loomward\v3` there
  once (a directory rename on one volume). It does nothing when the new home exists (no merge) or
  when `--state-dir` is given; a failed move refuses to start rather than open an empty store.
- **Release workflow** (`.github/workflows/release.yml`, not run here). The install smoke now
  asserts no `v3` inside the install directory, that the default start created
  `Loomward Data\v3\personal\state.db` (an error box keeps a failed start "up", so staying up alone
  proved nothing), then runs `uninstall.exe /S`, waits up to 120 s for the installed exe to go (the
  NSIS uninstaller re-runs itself from TEMP), and asserts `state.db` survived.
- **Probe** (`native/tests/webview2_probe.py`). The personal leg declines the confirmation first
  (expects `cancelled_by_user` and no root), then accepts it for the disposable folder only when the
  box names that folder. Both answers also record whether Cancel is the box's default push button.
  The `finally` block sets a stop flag for the driver and cancels (WM_CLOSE) any picker or
  confirmation the shell still shows before killing it.

## Tests and checks

| Check | Result |
|---|---|
| `cargo test --manifest-path native/Cargo.toml` | lib 4 passed; integration 32 passed, 1 ignored (the interactive disclosure box) |
| `cargo build` / `cargo clippy --all-targets -- -D warnings` / `cargo fmt --check` (native) | clean |
| `native/target/debug/loomward-desktop.exe --self-test` and the release build's | self-test passed (both) |
| `cargo fmt --all --check`, `cargo test --workspace --no-fail-fast` (851 passed), `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (no root-workspace code changed) |
| `py -3 scripts/verify.py` | exit 0 (227 Python tests, JS syntax and boundary, Rust format, tests and lint) |
| `py -3 native/tests/webview2_probe.py --out <scratch>` | 24/24, including "declining the native confirmation grants nothing (Cancel is the default)" and the accepted grant, scan and revocation of the disposable folder; P9 median 9.7 ms |
| Two real debug instances on one scratch state directory | second exits 1 with the `busy` message on stderr; after killing the first, a new instance stays up |
| Release (GUI subsystem) exe started from `pythonw` (no console, no std handles) while a debug instance holds the scratch state | the "Loomward could not start" box appears, owned by the second process, showing the `busy` reason; the process stays until the box is closed, then exits 1 |
| `release.yml` | YAML parses; the smoke step's PowerShell parses with no errors. Not run. |

New tests: `a_pick_grants_nothing_until_the_owner_confirms_that_folder` and the extended
`picker_keeps_the_main_window_handle` (dialogs.rs); `a_second_instance_on_one_state_directory_is_refused_until_the_first_closes`
and `state_kept_inside_the_install_directory_moves_out_once` (tests/desktop.rs). Updated:
`default_state_dir_is_per_user_and_per_dataset_class`, the default-launch test's state path, and
`a_synthetic_session_cannot_read_personal_state`, which now closes the personal instance before
pointing a synthetic one at its store (otherwise the lock refuses first).

## Mutations (each applied alone, the named test rerun, then reverted)

| Mutation | Killed by |
|---|---|
| lock never taken (`try_lock` replaced by `Ok`) | second-instance test |
| lock dropped as soon as the service opens | second-instance test |
| migration call removed | migration test |
| migration merges into an existing new home | migration test |
| default state back under `Loomward\v3` | `default_state_dir_...` test |
| confirmation skipped (`Some(folder)`) | confirmation test |
| no start directory on the picker | `picker_keeps_the_main_window_handle` |
| bidi and zero-width characters not escaped | confirmation test |
| letters outside ASCII escaped | confirmation test |

Not mutated: the error box's console test (`if !console`) needs a release rebuild per mutant; both
branches were observed instead (debug: stderr, no box; release from `pythonw`: box). The probe's
late-dialog cleanup was not exercised: no dialog outlived the driver in this run.

## Not verified

- The release workflow's install, uninstall and survival assertions have not run (not triggered
  here); whether `uninstall.exe /S` removes the exe within 120 s on the hosted runner is unmeasured.
- The migration against a real pre-#373 install, and a migration whose rename fails because an old
  build still holds the store open (the code refuses to start; no test drives it).
- A stray Enter on the real picker at the profile folder: reasoned from the folder-picker behaviour
  and the grant rules, not driven (it would pick the real profile).
- The in-app "granted: <folder>" notice from #373: the app has no grant control yet, so there is
  nowhere to show it.
- Focusing the first window from a second start.
