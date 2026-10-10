# Native shell evidence (lane L9, LW-018)

First build and first measurements of `loomward-desktop` (Tauri 2.12.2, wry 0.57.0, tao 0.37.1,
WebView2 154.0.4258.62), 2026-10-10, Windows 11 Pro 10.0.26300 x64, rustc 1.97.1, debug build.
Synthetic fixture service only; no real paths or names.

| File | What |
|---|---|
| `native-test-log.txt` | Every proving command with its output and exit code: `tauri info`, build, fmt, clippy, `cargo test` (7 + 1 ignored), the ignored real-MessageBox test, `--self-test`, the personal-mode refusals, the WebView2 probe, root `cargo test --workspace`, app check/test/build |
| `webview2-probe.json` | The real-window run of `native/tests/webview2_probe.py`: checks, ACL refusal messages, P9 runs, the shell's teardown log |
| `webview2-shell.log` | The shell's stderr during that run (teardown lines) |
| `desktop-home.png` | The shipped window on the tauri transport against the fixture service |
| `desktop-atlas-p9.png` | The Atlas after the P9 runs (mock transport, 2,500-node synthetic slice) |

**P9 in WebView2.** Layout + first draw of the 2,500-node Atlas slice, from the app's own
`loomward:atlas:*` performance marks: five runs 24.7, 11.1, 17.5, 12.0, 19.4 ms, median 17.5 ms
against the 50 ms budget, at 1440 x 1000 CSS px and device pixel ratio 1.25. Earlier runs the
same day gave medians of 20.6, 12.1 and 15.2 ms. The slice comes from the app's mock transport
(the fixture's `tree.slice` example is not 2,500 nodes), as in the Chromium P9 test. Hover and zoom
frame pacing were not measured in WebView2.

**Not covered here.** A release (`--release`, LTO) build; a native-dialog screenshot (the
MessageBox test drives the real box but captures nothing); the folder picker opening (only
reachable in a personal session, which the fixture refuses); revocation delivered by a real
`grants.revoke` (the fixture publishes a revoked `roots.changed` itself; the real path is L8);
Windows CI (L19).
