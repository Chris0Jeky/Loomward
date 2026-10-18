# Desktop picker follow-ups (#384)

Windows 11, KRASPYON, lane `fix/384-desktop-picker`, base `00bb610` (17–18 October 2026).
Compared the assigned surfaces with fetched `origin/main` at `f39b75d`: no changes there.
Synthetic fixtures and scratch state only. No commit, push, release or personal scan.

## Item-by-item reconciliation

| Finding on #384 | Current state / disposition |
| --- | --- |
| Architecture spec still names the install directory as the state home | Corrected docs/41 to the desktop's `Loomward Data` sibling. |
| Late grants never reach the list | Already reconciled through `roots.changed` and subsequent roots reads; retained and exercised. |
| Click before the initial roots load misattributes an old root | Already guarded in the view and store; retained regression. |
| Singleton keeps a stale Added message / tag after remount or revoke | Already cleared after revoke. Dismiss on leaving Health preserves a pending same-session offscreen reply; a context check before Health's first render retires receipts from another session. |
| Retry while the Windows dialog is open shows a busy failure | Already neutral for the specific open-dialog busy response; wording now says the dialog may still be open. Cooldown/capacity errors retain their own messages. |
| Timeout baseline never expires | Retire on leaving Health or on a roots read at least 60 seconds after the timeout/busy response. No new dialog is opened automatically. |
| Reply/fallback crosses a client, reconnect, engine or dataset change | Capture the client, connection/engine generation and session-info object; check before consuming replies and after fallback reads. Read through the captured client. Normal `roots.changed` does not invalidate a receipt. |
| Multiple roots can produce an arbitrary inferred receipt | Reconcile only a single new active root; multiple roots produce a generic list-changed notice with no root/tag. |
| Two test paths lose their backslashes | Corrected the string literals. |
| Release notes claim the interface cannot open the picker | Current RELEASING.md already describes Grants & health / Add a folder. The older evidence remains a historical snapshot. |
| Old and new state directories coexist silently | Startup notice names the current and retained legacy state homes. Both remain untouched by this notice. |
| No usable profile / no owner window looks like cancellation | Known absence of a configured native picker returns `capability_unavailable`. Declining an opened picker still reports cancellation; rfd itself still conflates OS dialog-open failure with cancellation. |
| Install/uninstall/state-survival assertions never ran hosted | Already resolved: [release run 38187436750](https://github.com/Chris0Jeky/Loomward/actions/runs/38187436750), head `7cf4a541`, passed on 12 October. Its smoke-test log reports state survived a silent uninstall at 00:26:33 UTC. No release triggered in this lane. |
| Observatory processTrends comment overstates its visible effect | Removed the overstated comment only. The cache reset and all behavior stay the same. |

## Verification

Red evidence: seven new grant-state cases failed before the initial fix (14 controls passed).
A separate changed-dataset case also failed before its fix (23 controls passed).
A delayed fallback-read expiry case failed before its correction (24 controls passed).
An `epoch_changed` lag notice arriving before the next hello also reproduced a stale receipt
(25 controls passed); that notice now retires the request immediately.
A completed offscreen receipt surviving adoption of another session also failed its return-to-Health
regression (26 controls passed). Health now checks receipt context before its first render/roots read.
The browser late-grant fixture's 0.8-second timer could skip observation of the intermediate deadline
state under load. It now delivers the grant explicitly after asserting that state and the empty list;
the original final Added/list assertions remain.
Native regressions first produced 34 passed, two expected failures and one ignored interactive test;
the fixed full native suite passes 60 tests with that same one ignored test.
Logs are local under `C:/lw/target/picker-384-*.log`.

Working directory: `C:/lw/wt/picker-384`. Every Cargo command uses
`CARGO_BUILD_JOBS=4` and `CARGO_TARGET_DIR=C:/lw/target/picker-384/native`.
The native lockfile also records the service's already-existing internal `loomward-ask` dependency;
no external dependency was added by this change.

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo fmt --manifest-path C:/lw/wt/picker-384/native/Cargo.toml --check` | PASS |
| `cargo test --manifest-path C:/lw/wt/picker-384/native/Cargo.toml -- --test-threads=4` | 60 passed, 0 failed, 1 existing ignored interactive test |
| `cargo build --manifest-path C:/lw/wt/picker-384/native/Cargo.toml` | PASS; final dependency record embeds app asset `index-B6rTlkyr.js` |
| `cargo clippy --manifest-path C:/lw/wt/picker-384/native/Cargo.toml --all-targets -- -D warnings` | PASS |
| `C:/lw/target/picker-384/native/debug/loomward-desktop.exe --self-test` | PASS, 14 checks, repeated with the final embedded app assets |
| `cargo test -p loomward-protocol -p loomward-service -- --test-threads=4` | 368 passed (54 protocol, 314 service), 2 pre-existing timing-sensitive deadline failures, 1 existing ignored; both failures pass individually below |
| `cargo test -p loomward-service --lib tests::a_duplicates_job_started_past_its_deadline_reports_the_started_job -- --exact --test-threads=1` | 1 passed, 0 failed |
| `cargo test -p loomward-service --lib tests::a_scan_started_past_its_deadline_reports_the_started_job -- --exact --test-threads=1` | 1 passed, 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace --no-fail-fast -- --test-threads=4` | 1,486 passed, 3 deadline failures, 2 existing ignored; not a green full-suite run. All three failed cases pass individually. |
| `C:/lw/target/picker-384/native/debug/deps/pathological-9e41ee6e2ea1ce1e.exe --exact past_writer_depth_limit_513_is_partial_with_depth_limit_reason --test-threads=1` (cwd `crates/loomward-engine`) | 1 passed, 0 failed, 9.87 s. The full-run failure was in `roots.forget` cleanup, after all depth/coverage assertions. |
| `npm.cmd --prefix C:/lw/wt/picker-384/app run check` | PASS, 0 errors / warnings |
| `npm.cmd --prefix C:/lw/wt/picker-384/app test -- --maxWorkers=2` | 500 passed, 47 files |
| `npm.cmd --prefix C:/lw/wt/picker-384/app run build` | PASS |
| Existing Chromium `check_add_folder` harness, rebuilt against final source | PASS: waiting, grants, cancellation, refusal, busy, deadline/late roots, no picker, offscreen completion and path-free requests |
| `py -3 C:/lw/wt/picker-384/scripts/test_app.py` | FAIL before picker scenarios: Atlas P9 hover 30 fps against 50 fps. No Atlas source changed; cause not established. No performance threshold weakened. |
| `git -C C:/lw/wt/picker-384 diff --check` | PASS after final native asset refresh |

Scoped Chromium command (same working directory):

```powershell
py -3 -u -c "import sys; sys.path.insert(0, 'C:/lw/wt/picker-384/scripts'); from test_app import build_app; from test_app_views import FakeEngine, check_add_folder, DIST; from playwright.sync_api import sync_playwright; build_app(DIST, development=True); engine = FakeEngine(); engine.start(); runner = sync_playwright().start(); browser = runner.chromium.launch(); page = browser.new_page(); check_add_folder(page, engine); browser.close(); runner.stop(); engine.stop()"
```

## Limits

Timeout reconciliation infers one newly active root from a bounded same-session list difference;
it is not a server-correlated receipt if another client adds a root inside that window.
The webview supplies no path, only the existing grant purpose. Rust owns selection and validation;
the existing grant result's display data is unchanged.
No new file/process effect or IPC command.

Real native reconnect/changed-dataset GUI behavior, real picker focus/default-button behavior,
and the startup notice's visual presentation require separate interactive proof. The Chromium
picker checks use the existing fake engine; native service tests use scratch stores and dialog stand-ins.
[HUMAN_TODO.md](../../HUMAN_TODO.md) q-45 remains the owner's hands-on checklist; it was not edited.
