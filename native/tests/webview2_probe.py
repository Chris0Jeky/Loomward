"""Drives the real loomward-desktop window (WebView2) over the Chrome DevTools Protocol.

Usage: py -3 native/tests/webview2_probe.py [--exe native/target/debug/loomward-desktop.exe]
                                            [--out evidence/v3/native] [--p9-runs 5]

Checks, in the shipped window and its real IPC:
  * the app loads from the embedded app/dist at http://tauri.localhost and selects the tauri transport;
  * lw_call answers; lw_events streams stream.hello, and repeats it after 15 s of silence (real heartbeat);
  * core/plugin commands (event, window, webview, app, path, dialog, fs, shell) are refused by the ACL;
  * the CSP is in force (an injected inline script is refused);
  * a page reload and a window close each tear the window's event streams down (the shell logs it);
  * P9: Atlas layout + first draw of the 2,500-node mock slice, from the app's performance marks.
Writes webview2-probe.json, the shell's stderr (webview2-shell.log) and screenshots. The remote
debugging port is opened only for this run, through WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS.
"""
from __future__ import annotations

import argparse
import ctypes
import json
import os
import socket
import statistics
import subprocess
import sys
import time
from ctypes import wintypes
from pathlib import Path

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[2]
HEARTBEAT_S = 15

OPEN_CHANNEL = """async () => {
  const t = window.__TAURI_INTERNALS__;
  window.__probe = [];
  const id = t.transformCallback((raw) => { if (!('end' in raw)) window.__probe.push({ at: performance.now(), m: raw.message }); });
  await t.invoke('lw_events', { channel: '__CHANNEL__:' + id, lastEpoch: null, lastSeq: null });
  return id;
}"""

DENIED = [
    ('plugin:event|listen', {'event': 'x', 'target': {'kind': 'Any'}, 'handler': 1}),
    ('plugin:window|close', {}),
    ('plugin:webview|create_webview_window', {}),
    ('plugin:app|version', {}),
    ('plugin:path|resolve_directory', {'directory': 1}),
    ('plugin:dialog|open', {}),
    ('plugin:fs|read_file', {'path': 'C:\\Windows\\win.ini'}),
    ('plugin:shell|execute', {}),
    ('lw_delete', {}),
]

P9_MARKS = """() => ['layout', 'first-paint', 'slice-to-paint'].map((k) =>
  performance.getEntriesByName(`loomward:atlas:${k}`).map((m) => ({ name: k, ms: m.duration, nodes: m.detail?.nodes, cells: m.detail?.cells }))).flat()"""


def free_port() -> int:
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def close_window(pid: int) -> bool:
    """Posts WM_CLOSE to the process's app window, as the title-bar close does."""
    user32 = ctypes.WinDLL('user32', use_last_error=True)
    found: list[int] = []
    proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    def each(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        cls = ctypes.create_unicode_buffer(64)
        user32.GetClassNameW(hwnd, cls, 64)
        # Only the app window: tao's hidden "Tao Thread Event Target" also reports visible, and
        # closing it wedges the event loop (a title-bar close never reaches it).
        if owner.value == pid and cls.value == 'Tauri Window':
            found.append(hwnd)
        return True

    user32.EnumWindows(proc(each), 0)
    for hwnd in found:
        user32.PostMessageW(hwnd, 0x0010, 0, 0)  # WM_CLOSE
    return bool(found)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--exe', type=Path, default=ROOT / 'native/target/debug/loomward-desktop.exe')
    ap.add_argument('--out', type=Path, default=ROOT / 'evidence/v3/native')
    ap.add_argument('--p9-runs', type=int, default=5)
    a = ap.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)
    port = free_port()
    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f'--remote-debugging-port={port}')
    log_path = a.out / 'webview2-shell.log'
    results: dict = {'checks': []}
    failed = 0

    def check(ok: bool, label: str, **detail) -> None:
        nonlocal failed
        failed += not ok
        results['checks'].append({'pass': bool(ok), 'check': label, **detail})
        print(('PASS ' if ok else 'FAIL ') + label + (f' {detail}' if detail else ''))

    with open(log_path, 'w', encoding='utf-8') as log:
        shell = subprocess.Popen([str(a.exe)], env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            with sync_playwright() as pw:
                browser = None
                for _ in range(60):
                    try:
                        browser = pw.chromium.connect_over_cdp(f'http://127.0.0.1:{port}')
                        break
                    except Exception:
                        time.sleep(0.5)
                if browser is None:
                    raise SystemExit('could not reach the WebView2 DevTools endpoint')
                results['webview2'] = browser.version
                page = next(p for c in browser.contexts for p in c.pages)
                page.wait_for_load_state('load')
                page.wait_for_timeout(1500)
                results['origin'] = page.evaluate('location.origin')
                check(results['origin'] == 'http://tauri.localhost', 'app served from the embedded dist', origin=results['origin'])
                check(page.evaluate("'__TAURI_INTERNALS__' in window"), 'the app sees the Tauri IPC and selects the tauri transport')
                page.screenshot(path=str(a.out / 'desktop-home.png'))
                inline = page.evaluate("""() => new Promise((resolve) => {
                  document.addEventListener('securitypolicyviolation', (e) => resolve(e.violatedDirective), { once: true });
                  const s = document.createElement('script'); s.textContent = 'window.__inline = 1'; document.head.appendChild(s);
                  setTimeout(() => resolve(window.__inline ? 'ran' : 'no violation reported'), 1000);
                })""")
                check(inline.startswith('script-src'), 'the CSP blocks an injected inline script', violated=inline)

                hello = page.evaluate("""() => window.__TAURI_INTERNALS__.invoke('lw_call', { request:
                    { protocol: 'loomward/3', request_id: 'r_probe', command: 'session.hello', payload: {} } })""")
                check(hello.get('ok') is True and hello['result']['adapter'] == 'tauri' and hello['result']['dataset_class'] == 'synthetic',
                      'lw_call session.hello over real IPC', adapter=hello.get('result', {}).get('adapter'))
                refused = page.evaluate("""() => window.__TAURI_INTERNALS__.invoke('lw_call', { request:
                    { protocol: 'loomward/3', request_id: 'r_probe2', command: 'roots.request_grant', payload: { purpose: 'metadata_scan' } } })""")
                check(refused['result']['refusal'] == 'synthetic_session_requires_lab_root',
                      'roots.request_grant refused in the synthetic session (no picker)', refusal=refused['result']['refusal'])

                denied = {}
                for cmd, args in DENIED:
                    r = page.evaluate("""async ([cmd, args]) => { try { await window.__TAURI_INTERNALS__.invoke(cmd, args); return null; }
                                          catch (e) { return String(e); } }""", [cmd, args])
                    denied[cmd] = r
                check(all(v is not None for v in denied.values()), 'every non-lw command is refused by the ACL', refusals=denied)

                page.evaluate(OPEN_CHANNEL)
                page.wait_for_function('() => window.__probe.length >= 1', timeout=5000)
                first = page.evaluate('window.__probe[0]')
                check(first['m']['event'] == 'stream.hello', 'lw_events: stream.hello first on the Channel')
                page.wait_for_function('() => window.__probe.length >= 2', timeout=(HEARTBEAT_S + 5) * 1000)
                beat = page.evaluate('window.__probe.slice(0, 2)')
                gap_s = (beat[1]['at'] - beat[0]['at']) / 1000
                check(beat[1]['m']['event'] == 'stream.hello' and HEARTBEAT_S - 0.5 <= gap_s <= HEARTBEAT_S + 2,
                      'stream.hello heartbeat after 15 s of silence on the real Channel', gap_s=round(gap_s, 2))

                page.reload()
                page.wait_for_load_state('load')
                page.wait_for_timeout(1000)
                log.flush()
                check('page load: closed' in log_path.read_text(encoding='utf-8'), 'a page reload tears down the old document\'s streams')

                # P9 inside WebView2: the mock transport's 2,500-node slice, as in app/tests/e2e/test_atlas.py.
                runs = []
                for i in range(a.p9_runs):
                    page.goto(f'http://tauri.localhost/index.html?transport=mock&run={i}#/atlas')
                    page.wait_for_function("() => performance.getEntriesByName('loomward:atlas:slice-to-paint').length > 0", timeout=15000)
                    marks = {m['name']: m for m in page.evaluate(P9_MARKS) if m['nodes'] == 2500}
                    runs.append({'layout_ms': round(marks['layout']['ms'], 2), 'first_paint_ms': round(marks['first-paint']['ms'], 2),
                                 'layout_plus_paint_ms': round(marks['layout']['ms'] + marks['first-paint']['ms'], 2),
                                 'slice_to_paint_wall_ms': round(marks['slice-to-paint']['ms'], 2), 'cells': marks['layout']['cells']})
                page.wait_for_timeout(2000)
                page.screenshot(path=str(a.out / 'desktop-atlas-p9.png'))
                med = statistics.median(r['layout_plus_paint_ms'] for r in runs)
                results['p9'] = {'nodes': 2500, 'runs': runs, 'median_layout_plus_paint_ms': med, 'budget_ms': 50,
                                 'viewport': page.evaluate('[innerWidth, innerHeight, devicePixelRatio]')}
                check(med <= 50, 'P9 layout + first draw of 2,500 nodes within 50 ms in WebView2 (median)', median_ms=med)

                # Back on the tauri transport with a probe stream open, then close the window.
                page.goto('http://tauri.localhost/index.html')
                page.wait_for_load_state('load')
                page.evaluate(OPEN_CHANNEL)
                page.wait_for_function('() => window.__probe.length >= 1', timeout=5000)
                log.flush()
                before = log_path.read_text(encoding='utf-8').count('window close: closed')
                check(close_window(shell.pid), 'WM_CLOSE posted to the shell window')
                shell.wait(timeout=20)
                check(shell.returncode == 0, 'the shell exits cleanly after the window closes', code=shell.returncode)
        finally:
            if shell.poll() is None:
                shell.kill()
    text = log_path.read_text(encoding='utf-8')
    check(text.count('window close: closed') > before, 'window close tears down the open streams')
    results['shell_log'] = text.splitlines()
    (a.out / 'webview2-probe.json').write_text(json.dumps(results, indent=2) + '\n', encoding='utf-8')
    print(f'{len(results["checks"]) - failed}/{len(results["checks"])} passed')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
