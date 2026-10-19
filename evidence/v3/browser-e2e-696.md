# Browser reconnect and refused connections (#696)

Windows 11, KRASPYON, 2026-10-19. Starting HEAD:
`aa92edc96a0b032257f30fe5cd37326300e4f360`. Changes are deliberately
uncommitted on `fix/696-e2e-reconnect`.

## Reconnect: a response from the previous document

The original focused browser sequence reproduced the timeout on its second cycle:
the recovery was connected, but the fake engine had recorded no resume IDs.
Waiting longer after recovery cannot manufacture a reconnect that never happened.

A discriminating probe delayed SSE response headers by 300 ms and allowed the
previous document's response to arrive in the reload waiter's observation window
(400 ms before dispatching the reload). It recorded request object identities:

```text
Original ordering: MATCHED STREAM INDEX 0 OF 1
Recovery: PROBE 0 []
AssertionError: timed out waiting for ... check_resume

Wait for the initial stream before reloading: MATCHED STREAM INDEX 1 OF 2
Recovery: PROBE 0 ['e_fake_epoch.0']
PASS the event stream resumes with the epoch-qualified id
```

The old waiter accepted the old document's SSE response. It did not establish
that the replacement document had subscribed before the test took the engine
down. Its first successful stream could therefore be the recovery itself. The
new initial-response wait removes that ambiguity; the existing reload-response
wait and resume-ID assertions remain. An unperturbed focused control passed
eight cycles. The transport unit check also pins the correct behavior when a
connection ends before its first frame: no invented Last-Event-ID, followed by
epoch-qualified resume once a hello has actually arrived. No production client
change was needed.

## Connection refused: the fake HTTP listener's backlog

Adding the console location captured the real full-suite failure at
`http://127.0.0.1:56077/api/v3/call`. It was a failed API resource, not an
unhandled JavaScript exception. No deliberately stopped-engine request is needed
to reproduce it: Python's `ThreadingHTTPServer.request_queue_size` is five here.
Chromium alone can use six HTTP/1 connections; old and replacement documents can
overlap. Slow accept/dispatch under host load fills that small listener queue.

A browser probe held the dispatcher for two seconds and sent twelve concurrent
POSTs to the still-open listener. It changed only the listen backlog:

| Listen backlog | Successful calls | Failed fetches | Console errors |
| --- | ---: | ---: | ---: |
| 5 (stdlib default) | 5 | 7 | 7 |
| 128 | 12 | 0 | 0 |

The failures were exactly `Failed to load resource: net::ERR_CONNECTION_REFUSED`,
with `location.url = http://127.0.0.1:49309/api/v3/call`. The listener remained
open throughout. This rules out a dead port as a necessary cause and reproduces
the Windows failure through Chromium's actual network stack. Retrying a
temporarily unreachable engine is correct client behavior.

Both fake engines now use a shared test-only server with a backlog of 128. The
real-socket regression holds dispatch altogether and queues twelve connections
against each fixture's actual server constructor. Both subcases timed out with
the old imports; both pass with the shared server. No request or console message
is ignored, and no production client change is needed. The final `not errors`
assertion remains intact; its diagnostics now include `location.url`.

## Verification

Setup used `npm.cmd --prefix app ci`, then `npm.cmd --prefix app run build`.
The global `py -3` interpreter lacked Playwright. A worktree-local `.venv` was
created with `py -3 -m venv .venv`, and `.venv/Scripts/python.exe -m pip install
playwright` installed Playwright 1.64.0 on Python 3.14.7. Its matching Chromium
1248 was already installed; no browser download or host configuration change was
needed. Browser commands below use that explicit interpreter.

The unchanged baseline first failed the P9 hover budget at 30 fps (minimum 50),
then a diagnostic full run passed. Intermediate repeats before the backlog fix
were 2 full passes, 2 P9 failures (hover/zoom at 30 fps), and 1 refused API call.
Those results are retained separately from final-patch repeats. No performance
budget was changed and `CI` report-only mode was not used.

Final-patch repeats (`.venv/Scripts/python.exe -u scripts/test_app.py`):

| Run | Result | Passing assertions | Cargo active at reconnect |
| --- | --- | ---: | --- |
| 1 | FAIL: P9 hover measured 30 fps, minimum 50 | 22 | Not reached |
| 2 | ALL PASS | 284 | Yes |
| 3 | ALL PASS | 284 | Yes |
| 4 | ALL PASS | 284 | No |
| 5 | ALL PASS | 284 | No |

Thus **4/5 full passes**, and **4/4 reached reconnect and final console hygiene
checks pass**. Runs 2 and 3 each overlapped an actively compiling
`cargo build --manifest-path C:/lw/wt/w12-696/Cargo.toml -p loomward-catalog`.
The build processes used `CARGO_PROFILE_DEV_DEBUG=1` and `0`, respectively, to
exercise fresh compilations in the same lane target; both builds passed. All
Cargo commands used `CARGO_BUILD_JOBS=4` and
`CARGO_TARGET_DIR=C:/lw/target/lane-w12-696`. Workspace tests additionally used
`RUST_TEST_THREADS=3`.

Commands below ran from this worktree (absolute paths used for tool calls):

| Command | Result |
| --- | --- |
| `npm.cmd --prefix app ci` | PASS |
| `npm.cmd --prefix app run check` | PASS, 0 errors and 0 warnings |
| `npm.cmd --prefix app run test -- --maxWorkers=2` | PASS, 612 tests in 50 files |
| `npm.cmd --prefix app run build` | PASS |
| `.venv/Scripts/python.exe scripts/test_app_timing.py` | PASS, 6 tests, including both real-socket subcases |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace --no-fail-fast` | PASS, 1,727 passed, 4 ignored, 0 failed |
| `git -C C:/lw/wt/w12-696 diff --check` | PASS |

The existing ignored Rust cases are the million-row detector benchmark, 1 GiB
streaming-memory probe, 20k durable-stop lab, and throughput lab. They were not
enabled. Hosted/Linux CI, live-serve and the desktop shell were not run.

Raw probe and repeat logs are retained under the ignored `.loomward/` directory:
`delayed-race.log`, `delayed-control.log`, `backlog.log`,
`final-e2e-results.json`, `final-e2e-{1..5}.log`,
`final-parallel-cargo-{2,3}.log`, `app-{check,test,build}.log`, `clippy.log`,
and `workspace-test.log`. The final repeat runner exits nonzero because run 1
failed; its four later successes do not erase that failure. The P9 performance
issue is left outside this repair. No production transport code changed.

`HUMAN_TODO.md` is unchanged. This lane needs no owner decision and changes no
product capability or safety gate. Native-shell and hosted-CI proof are outside
this uncommitted browser-test lane.
