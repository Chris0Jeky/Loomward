"""Drives the real loomward-desktop window (WebView2) over the Chrome DevTools Protocol.

Usage: py -3 native/tests/webview2_probe.py [--exe native/target/debug/loomward-desktop.exe]
                                            [--out evidence/v3/native] [--p9-runs 5]

Checks, in the shipped window and its real IPC:
  * the app loads from the embedded app/dist at http://tauri.localhost and selects the tauri transport;
  * lw_call answers; lw_events streams stream.hello, and repeats it after 15 s of silence (real heartbeat);
  * core/plugin commands (event, window, webview, app, path, dialog, fs, shell) are refused by the ACL;
  * the CSP is in force (an injected inline script is refused);
  * a page reload and a window close each tear the window's event streams down (the shell logs it);
  * raw IPC bounds and duplicate keys, blocked navigation/popups, production mock denial, renderer lag.
  * P9: 2,500-node Atlas layout + first draw in a separate development fixture shell build.
Writes webview2-probe.json, the shell's stderr (webview2-shell.log) and screenshots. The remote
debugging port is opened only for this run, through WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS.
"""
from __future__ import annotations

import argparse
import ctypes
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from ctypes import wintypes
from pathlib import Path

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[2]
HEARTBEAT_S = 15
PROFILES = ROOT / '.loomward/webview2-profiles'

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


def fixture_shell() -> Path:
    """Embed development fixtures without changing app/dist or leaving a fixture production binary."""
    fixture = ROOT / '.loomward/webview2-fixture'
    dist = fixture / 'app'
    subprocess.run(
        ['npm.cmd' if os.name == 'nt' else 'npm', '--prefix', 'app', 'run', 'build', '--', '--outDir', str(dist)],
        cwd=ROOT, env=dict(os.environ, NODE_ENV='development'), check=True,
    )
    build = ['cargo', 'build', '--manifest-path', 'native/Cargo.toml', '--target-dir', 'native/target']
    # Tauri can parse a Windows drive prefix as a URL; use its config-relative directory form.
    env = dict(os.environ, TAURI_CONFIG=json.dumps({'build': {'frontendDist': os.path.relpath(dist, ROOT / 'native')}}))
    exe = fixture / 'loomward-desktop.exe'
    try:
        subprocess.run(build, cwd=ROOT, env=env, check=True)
        shutil.copy2(ROOT / 'native/target/debug/loomward-desktop.exe', exe)
    finally:
        subprocess.run(build, cwd=ROOT, check=True)
    return exe


def connect_webview(pw, profile: Path):
    """WebView2 picks its own debug port (no pre-bound port to race) and writes it to DevToolsActivePort."""
    port = None
    for _ in range(60):
        marker = next(profile.rglob('DevToolsActivePort'), None)
        try:
            port = port or int(marker.read_text().split()[0])
            return pw.chromium.connect_over_cdp(f'http://127.0.0.1:{port}')
        except Exception:
            time.sleep(0.5)
    found = 'no DevToolsActivePort' if port is None else f'DevToolsActivePort {port} refused'
    raise RuntimeError(f'could not reach the WebView2 DevTools endpoint ({found})')


def p9(exe: Path, out: Path, count: int) -> dict:
    PROFILES.mkdir(parents=True, exist_ok=True)
    profile = Path(tempfile.mkdtemp(prefix='p9-profile-', dir=PROFILES))
    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=0',
               WEBVIEW2_USER_DATA_FOLDER=str(profile.resolve()))
    with open(out / 'webview2-p9-shell.log', 'w', encoding='utf-8') as log:
        shell = subprocess.Popen([str(exe)], env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            with sync_playwright() as pw:
                browser = connect_webview(pw, profile)
                page = next(p for c in browser.contexts for p in c.pages)
                # Let Tauri's initial navigation finish before replacing it with the fixture URL.
                page.wait_for_selector('[data-mode]')
                runs = []
                for i in range(count):
                    page.goto(f'http://tauri.localhost/index.html?transport=mock&run={i}#/atlas')
                    page.wait_for_function("() => performance.getEntriesByName('loomward:atlas:slice-to-paint').length > 0", timeout=15000)
                    assert page.locator('[data-mode=mock]').count() == 1, 'P9 requires the development fixture transport'
                    marks = {m['name']: m for m in page.evaluate(P9_MARKS) if m['nodes'] == 2500}
                    assert {'layout', 'first-paint', 'slice-to-paint'} <= marks.keys(), 'P9 requires all 2,500-node timing marks'
                    runs.append({'layout_ms': marks['layout']['ms'], 'first_paint_ms': marks['first-paint']['ms'],
                                 'layout_plus_paint_ms': marks['layout']['ms'] + marks['first-paint']['ms'],
                                 'slice_to_paint_wall_ms': marks['slice-to-paint']['ms'], 'cells': marks['layout']['cells']})
                page.screenshot(path=str(out / 'desktop-atlas-p9.png'))
                result = {'nodes': 2500, 'runs': runs, 'median_layout_plus_paint_ms': statistics.median(r['layout_plus_paint_ms'] for r in runs),
                          'budget_ms': 50, 'viewport': page.evaluate('[innerWidth, innerHeight, devicePixelRatio]')}
                assert close_window(shell.pid), 'P9 window did not close'
                shell.wait(timeout=20)
                assert shell.returncode == 0, 'P9 shell did not exit cleanly'
                return result
        finally:
            if shell.poll() is None:
                shell.kill()
                shell.wait(timeout=20)

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
    if a.p9_runs < 1:
        ap.error('--p9-runs must be positive')
    a.out.mkdir(parents=True, exist_ok=True)
    # Each run owns its profile: debug options must not collide with another WebView2 environment.
    PROFILES.mkdir(parents=True, exist_ok=True)
    profile = Path(tempfile.mkdtemp(prefix='profile-', dir=PROFILES))
    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=0',
               WEBVIEW2_USER_DATA_FOLDER=str(profile.resolve()))
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
                try:
                    browser = connect_webview(pw, profile)
                except RuntimeError as e:
                    hosts = subprocess.run(['tasklist', '/FI', 'IMAGENAME eq msedgewebview2.exe', '/FO', 'CSV', '/NH'],
                                           capture_output=True, text=True).stdout.count('msedgewebview2.exe')
                    raise RuntimeError(f'{e}; shell exit code {shell.poll()}, {hosts} msedgewebview2 processes') from e
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

                hello = page.evaluate("""() => window.__TAURI_INTERNALS__.invoke('lw_call', new TextEncoder().encode(JSON.stringify(
                    { protocol: 'loomward/3', request_id: 'r_probe', command: 'session.hello', payload: {} })))""")
                check(hello.get('ok') is True and hello['result']['adapter'] == 'tauri' and hello['result']['dataset_class'] == 'synthetic',
                      'lw_call session.hello over real IPC', adapter=hello.get('result', {}).get('adapter'))
                refused = page.evaluate("""() => window.__TAURI_INTERNALS__.invoke('lw_call', new TextEncoder().encode(JSON.stringify(
                    { protocol: 'loomward/3', request_id: 'r_probe2', command: 'roots.request_grant', payload: { purpose: 'metadata_scan' } })))""")
                check(refused['result']['refusal'] == 'synthetic_session_requires_lab_root',
                      'roots.request_grant refused in the synthetic session (no picker)', refusal=refused['result']['refusal'])

                raw = page.evaluate("""async () => {
                  const invoke = window.__TAURI_INTERNALS__.invoke;
                  const base = JSON.stringify({protocol:'loomward/3',request_id:'r_raw',command:'session.hello',payload:{}});
                  const bytes = (s) => new TextEncoder().encode(s);
                  const exact = await invoke('lw_call', bytes(base.padEnd(65536, ' ')));
                  let oversized = false, parsed = false;
                  try { await invoke('lw_call', bytes(base.padEnd(65537, ' '))); } catch { oversized = true; }
                  try { await invoke('lw_call', {request:JSON.parse(base)}); } catch { parsed = true; }
                  const dup = await invoke('lw_call', bytes(base.replace('"payload":{}', '"payload":{"x":1,"x":2}')));
                  return {exact:exact.ok,oversized,parsed,duplicate:!dup.ok && dup.error.message.includes('duplicate')};
                }""")
                check(all(raw.values()), 'raw IPC: exact 64 KiB accepted, excess/parsed bodies/duplicates refused', **raw)

                page.evaluate("location.href = 'https://remote.invalid/'")
                page.wait_for_timeout(300)
                check(page.url.startswith('http://tauri.localhost/'), 'navigation to a remote origin is denied')
                pages_before = sum(len(c.pages) for c in browser.contexts)
                page.evaluate("window.open('https://remote.invalid/', '_blank')")
                page.wait_for_timeout(300)
                check(sum(len(c.pages) for c in browser.contexts) == pages_before, 'new windows are denied')

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

                page.goto('http://tauri.localhost/index.html?transport=mock')
                page.wait_for_function("() => document.querySelector('[data-mode=tauri]')", timeout=5000)
                check(page.locator('[data-mode=tauri]').count() == 1, 'production ignores the mock transport override')

                # Exercise the compiled transport, then stall this renderer across a heartbeat.
                module = next((ROOT / 'app/dist/assets').glob('tauri-*.js')).name
                page.evaluate("""async (module) => {
                  const exports = await import('/assets/' + module);
                  const create = Object.values(exports).find((v) => typeof v === 'function');
                  const transport = await create();
                  window.__stall = [];
                  window.__stallStop = transport.subscribe((e) => window.__stall.push(e), () => {});
                }""", module)
                page.wait_for_function('() => window.__stall.length > 0', timeout=5000)
                page.evaluate('() => { const end = performance.now() + 22000; while (performance.now() < end) {} }')
                page.wait_for_function("() => window.__stall.some((e) => e.event === 'stream.lagged')", timeout=5000)
                check(page.evaluate("window.__stall.some((e) => e.event === 'stream.lagged' && e.data.reason === 'subscriber_overflow')"),
                      'stalled renderer receives stream.lagged and refetch instructions')
                page.evaluate('window.__stallStop()')

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
    try:
        results['p9'] = p9(fixture_shell(), a.out, a.p9_runs)
        med = results['p9']['median_layout_plus_paint_ms']
        check(med <= 50, 'P9 layout + first draw of 2,500 nodes within 50 ms in WebView2 (median)', median_ms=round(med, 2))
    except Exception as e:
        check(False, 'P9 development fixture measurement completed', error=type(e).__name__)
    (a.out / 'webview2-probe.json').write_text(json.dumps(results, indent=2) + '\n', encoding='utf-8')
    print(f'{len(results["checks"]) - failed}/{len(results["checks"])} passed')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
