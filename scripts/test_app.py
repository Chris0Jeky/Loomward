"""Browser checks for the Svelte app in app/ (lane L3). Requires Playwright and a Chromium installation.

Serves the built app/dist from a tiny loopback static server and drives it with Chromium:
  - mock mode (?transport=mock): shell chrome, labels, both themes, hostile names as text, routes;
  - no engine: "unavailable", never demo data;
  - http mode against a small in-process fake engine: token header, fragment scrubbed, SSE held open,
    and a dropped engine flips the shell to "unavailable" and back.
Run `npm.cmd --prefix app run build` first.

`--live-serve` replaces the Python servers with the real `loomward-serve --static app/dist` binary
(lane L6, its FixtureService answering from contracts/v3/examples): the static files under the
server's own CSP header, the fragment-token handshake, calls and the SSE stream through the real
security boundary, and "unavailable" once the process is gone.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
from types import SimpleNamespace
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import Page, expect, sync_playwright

from test_app_views import run_views

ROOT = Path(__file__).resolve().parents[1]
TOKEN = 'a1b2c3d4e5f60718293a4b5c6d7e8f90'
HOSTILE = '<img src=x onerror=alert(1)>.png'
MIME = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.json': 'application/json'}


class Engine:
    """Flags the test flips; a stand-in for loomward-serve answering the few commands the shell needs."""

    def __init__(self) -> None:
        self.down = False
        self.tokens_seen: set[str] = set()
        self.dataset = 'personal'
        self.calls: list[str] = []


def envelope(req: dict, result=None, error=None) -> dict:
    base = {'protocol': 'loomward/3', 'request_id': req['request_id']}
    if error:
        return {**base, 'ok': False, 'error': {'code': error, 'message': f"fake engine: {req['command']} not implemented", 'retryable': False, 'detail': None}}
    return {**base, 'ok': True, 'result': result, 'meta': {'served_at': '2026-10-01T00:00:00Z', 'elapsed_ms': 0, 'dataset_class': engine.dataset, 'budget_hit': False}}


def fake_result(command: str):
    if command == 'session.hello':
        effects = {k: False for k in 'file_move file_delete file_rename file_write content_read process_kill process_suspend process_priority memory_trim uninstall elevation'.split()}
        return {
            'protocol': 'loomward/3', 'engine_version': 'fake-engine', 'adapter': 'http', 'dataset_class': engine.dataset,
            'session_started_at': '2026-10-01T00:00:00Z', 'enumeration_strategy': 'std_read_dir',
            'capabilities': {'observation': {}, 'effects': effects},
            'features': {k: False for k in ('grant_picker', 'disclosure_dialog', 'teacher_available', 'telemetry_available', 'gpu_available')},
            'limits': {'max_request_bytes': 65536, 'max_slice_nodes': 6000, 'max_page_items': 200},
        }
    if command == 'roots.list':
        return {'roots': []}
    if command == 'grants.list':
        return {'grants': []}
    return None


engine = Engine()
DIST = ROOT / 'app' / 'dist'


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'

    def log_message(self, *a):  # quiet
        pass

    def _send(self, status: int, body: bytes, ctype: str, extra: dict | None = None) -> None:
        self.send_response(status)
        self.send_header('Content-Type', ctype)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'no-store')
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def _authorised(self) -> bool:
        token = self.headers.get('X-Loomward-Token', '')
        if token != TOKEN:
            self._send(403, b'{}', 'application/json')
            return False
        engine.tokens_seen.add(token)
        return True

    def do_GET(self) -> None:
        path = urlsplit(self.path).path
        if path == '/api/v3/events':
            if engine.down:
                return self._send(503, b'{}', 'application/json')
            if not self._authorised():
                return
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Cache-Control', 'no-store')
            self.end_headers()
            try:
                self.wfile.write(b'id: 0\nevent: stream.hello\ndata: ' + json.dumps({'protocol': 'loomward/3', 'seq': 0, 'event': 'stream.hello', 'at': '2026-10-01T00:00:00Z', 'data': {}}).encode() + b'\n\n')
                self.wfile.flush()
                while not engine.down:
                    self.wfile.write(b': heartbeat\n\n')
                    self.wfile.flush()
                    time.sleep(0.1)
            except OSError:
                pass
            return
        file = DIST / ('index.html' if path in ('/', '') else path.lstrip('/'))
        try:
            file = file.resolve()
            if DIST.resolve() not in file.parents and file != DIST.resolve() / 'index.html':
                raise FileNotFoundError
            self._send(200, file.read_bytes(), MIME.get(file.suffix, 'application/octet-stream'))
        except (FileNotFoundError, OSError):
            self._send(404, b'not found', 'text/plain')

    def do_POST(self) -> None:
        if urlsplit(self.path).path != '/api/v3/call':
            return self._send(405, b'', 'text/plain')  # the static-only case: no engine behind this origin
        body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        if engine.down:
            return self._send(503, b'{}', 'application/json')
        if not self._authorised():
            return
        req = json.loads(body)
        engine.calls.append(req['command'])
        result = fake_result(req['command'])
        out = envelope(req, result) if result is not None else envelope(req, error='capability_unavailable')
        self._send(200, json.dumps(out).encode(), 'application/json')


def check(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)
    print('PASS', msg)


def theme_snapshot(page: Page) -> dict:
    return page.evaluate("""() => {
      const cs = getComputedStyle(document.documentElement);
      return { theme: document.documentElement.dataset.theme, bg: cs.backgroundColor,
               num: getComputedStyle(document.querySelector('.chip strong')).fontFamily,
               meaning: cs.getPropertyValue('--meaning').trim(), residency: cs.getPropertyValue('--residency').trim(),
               permission: cs.getPropertyValue('--permission').trim() };
    }""")


def run_live_serve(browser_path: str | None) -> None:
    """The app's http transport against the real loomward-serve binary (lane L6)."""
    subprocess.run(['cargo', 'build', '-q', '-p', 'loomward-http', '--bin', 'loomward-serve'], cwd=ROOT, check=True)
    exe = ROOT / 'target' / 'debug' / ('loomward-serve.exe' if sys.platform == 'win32' else 'loomward-serve')
    proc = subprocess.Popen([str(exe), '--static', str(DIST)], stdout=subprocess.PIPE, text=True)
    try:
        url = (proc.stdout.readline() if proc.stdout else '').strip()
        check(url.startswith('http://127.0.0.1:') and '/#token=' in url, 'loomward-serve printed the fragment-token URL')
        base, token = url.split('/#token=')
        check(len(token) == 64, 'the session token is 32 random bytes, hex encoded')
        with sync_playwright() as pw:
            browser = pw.chromium.launch(executable_path=browser_path, headless=True, args=['--no-sandbox'])
            page = browser.new_context(viewport={'width': 1360, 'height': 900}).new_page()
            page.set_default_timeout(8000)
            errors: list[str] = []
            hosts: set[str] = set()
            page.on('pageerror', lambda e: errors.append(f'pageerror: {e}'))
            page.on('console', lambda m: m.type == 'error' and errors.append(f'console: {m.text}'))
            page.on('request', lambda r: hosts.add(urlsplit(r.url).hostname or r.url))

            res = page.goto(f'{base}/?transport=mock')
            headers = res.headers if res else {}
            check("script-src 'self'" in headers.get('content-security-policy', '') and headers.get('x-content-type-options') == 'nosniff', 'static page carries the CSP and nosniff headers')
            expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
            print('PASS the built app runs under the server CSP (mock mode)')

            page.evaluate('sessionStorage.clear()')
            page.goto(url)
            status = page.get_by_role('status', name='Session status')
            expect(status).to_contain_text('Connected')
            check('token' not in page.url, 'the token is scrubbed from the address bar')
            expect(status).to_contain_text('Dataset synthetic')
            expect(status).to_contain_text('Browser')
            check(page.evaluate(f"""async () => (await fetch('/api/v3/call', {{method: 'POST', body: '{{}}', headers: {{'Content-Type': 'application/json'}}}})).status""") == 403, 'a call without the token header is refused')
            page.wait_for_timeout(1500)  # the SSE stream stays open: still connected
            expect(status).to_contain_text('Connected')
            print('PASS http transport connected through loomward-serve')

            proc.terminate()
            proc.wait(timeout=10)
            expect(page.get_by_role('alert')).to_contain_text('Unavailable', timeout=15000)
            expect(status).to_contain_text('Dataset unknown')
            print('PASS a stopped server flips the shell to unavailable')

            check(hosts <= {'127.0.0.1'}, f'every request stayed on loopback: {sorted(hosts)}')
            unexpected = [e for e in errors if 'Failed to load resource' not in e and 'ERR_CONNECTION_REFUSED' not in e]
            check(not unexpected, f'no console errors or page errors: {unexpected}')
            browser.close()
    finally:
        if proc.poll() is None:
            proc.kill()
    print('ALL PASS (live serve)')


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument('--browser', default=None, help='path to a Chromium executable')
    ap.add_argument('--screenshots', type=Path)
    ap.add_argument('--view-shots', type=Path, help='directory for view screenshots and results from both view lanes (evidence/v3/app-views); omitted = none')
    ap.add_argument('--live-serve', action='store_true', help='run the http leg against the real loomward-serve binary')
    a = ap.parse_args()
    if not (DIST / 'index.html').exists():
        sys.exit('app/dist is missing: run `npm.cmd --prefix app run build` first')
    if a.live_serve:
        return run_live_serve(a.browser)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{server.server_port}'
    shots = a.screenshots
    if shots:
        shots.mkdir(parents=True, exist_ok=True)

    with sync_playwright() as pw:
        browser = pw.chromium.launch(executable_path=a.browser, headless=True, args=['--no-sandbox'])
        context = browser.new_context(viewport={'width': 1360, 'height': 900})
        page = context.new_page()
        page.set_default_timeout(8000)
        errors: list[str] = []
        hosts: set[str] = set()
        dialogs: list[str] = []
        page.on('pageerror', lambda e: errors.append(f'pageerror: {e}'))
        page.on('console', lambda m: m.type == 'error' and errors.append(f'console: {m.text}'))
        page.on('dialog', lambda d: (dialogs.append(d.message), d.dismiss()))
        page.on('request', lambda r: hosts.add(urlsplit(r.url).hostname or r.url))

        def shot(name: str) -> None:
            if shots:
                page.wait_for_timeout(300)
                page.screenshot(path=str(shots / name), full_page=True)

        # --- mock mode ---------------------------------------------------------------------------
        page.goto(f'{base}/?transport=mock')
        expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
        status = page.get_by_role('status', name='Session status')
        expect(status).to_contain_text('Dataset synthetic')
        expect(status).to_contain_text('Simulation')
        expect(status).to_contain_text('Mock transport')
        expect(status).to_contain_text('Connected')
        check(page.url.endswith('#/explorer'), 'default route is the first discovered view (#/explorer)')
        nav = page.get_by_role('navigation', name='Views')
        check(nav.get_by_role('link').count() >= 2, 'views are discovered into the navigation')
        expect(nav.get_by_role('link', name='Explorer')).to_have_attribute('aria-current', 'page')

        rows = page.locator('main table tbody tr')
        expect(rows.first).to_be_visible()
        first_page = rows.count()
        check(first_page > 0, f'explorer lists rows from the mock transport ({first_page})')
        check(page.get_by_text(HOSTILE, exact=True).count() == 1, 'hostile file name is on screen as literal text')
        check(page.locator('main img, main svg, main script, main iframe').count() == 0, 'hostile names created no elements')
        check(page.get_by_text('<script>alert(document.domain)</script>.txt', exact=True).count() == 1, 'script-tag name rendered as text')
        check(not dialogs, 'no script from a file name ran')
        check('unknown' in page.locator('main table').inner_text(), 'unknown allocation stays "unknown" in the table')
        shot('woven-atlas-explorer.png')

        # search pages with a keyset cursor: 'a' matches far more than one page
        page.get_by_placeholder('Search names').fill('a')
        page.get_by_role('button', name='Search', exact=True).click()
        expect(page.get_by_text('Search results for')).to_be_visible()
        expect(page.locator('main table tbody tr')).to_have_count(50)
        page.get_by_role('button', name='Load more').click()
        expect(page.locator('main table tbody tr')).to_have_count(100)
        print('PASS search pages with Load more')

        page.get_by_placeholder('Search names').fill('script')
        page.get_by_role('button', name='Search', exact=True).click()
        expect(page.locator('main table tbody tr').first).to_contain_text('script')
        page.get_by_role('button', name='Clear search').click()
        expect(page.locator('main nav[aria-label="Location"]')).to_be_visible()

        # drill down, then breadcrumb back
        page.locator('main table tbody tr td button.crumb').first.click()
        expect(page.locator('main nav[aria-label="Location"] button')).to_have_count(2)
        page.locator('main nav[aria-label="Location"] button').first.click()
        expect(page.locator('main nav[aria-label="Location"] button')).to_have_count(1)
        print('PASS drill down and breadcrumb back')

        # --- routes ----------------------------------------------------------------------------
        page.get_by_role('navigation', name='Views').get_by_role('link', name='Grants & health').click()
        expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
        expect(page.get_by_text('Synthetic Alpha', exact=False).first).to_be_visible()
        check(page.get_by_text('off', exact=True).count() == 11, 'all 11 effect capabilities show as off')
        check(page.get_by_text('unknown').count() >= 3, 'engine figures the mock cannot know are shown as unknown')
        page.evaluate("location.hash = '#/nope'")
        expect(page.get_by_role('heading', name='No such view')).to_be_visible()
        page.evaluate("location.hash = '#/explorer'")
        expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
        print('PASS routes: view, unknown view, back')

        # --- view modules (lane L10): app/tests/e2e/test_<view>.py, each owned by its view's lane ----
        if a.view_shots:
            a.view_shots.mkdir(parents=True, exist_ok=True)

        def view_shot(name: str, full: bool = True) -> None:
            if not a.view_shots:
                return
            page.wait_for_timeout(200)
            page.screenshot(path=str(a.view_shots / name), full_page=full)

        ctx = SimpleNamespace(page=page, base=base, check=check, dialogs=dialogs, view_shot=view_shot)
        view_results = {}
        for mod_path in sorted((ROOT / 'app' / 'tests' / 'e2e').glob('test_*.py')):
            spec = importlib.util.spec_from_file_location(mod_path.stem, mod_path)
            mod = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(mod)
            print(f'--- {mod_path.name}')
            view_results[mod_path.stem] = mod.run(ctx)
        if a.view_shots:
            (a.view_shots / 'results.json').write_text(json.dumps(view_results, indent=2) + chr(10), encoding='utf-8')
        page.set_viewport_size({'width': 1360, 'height': 900})
        page.goto(f'{base}/?transport=mock#/explorer')
        expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()

        # --- themes ----------------------------------------------------------------------------
        atlas = theme_snapshot(page)
        page.get_by_role('button', name='Observatory').click()
        obs = theme_snapshot(page)
        check(atlas['theme'] == 'woven-atlas' and obs['theme'] == 'observatory', 'theme toggle switches data-theme')
        check(atlas['bg'] != obs['bg'], 'the two themes have different backgrounds')
        check(len({obs['meaning'], obs['residency'], obs['permission']}) == 3 and len({atlas['meaning'], atlas['residency'], atlas['permission']}) == 3, 'three distinct thread colours in each theme')
        check('Cascadia' in obs['num'], 'observatory numerics use the monospace face')
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Dataset synthetic')
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Simulation')
        shot('observatory-explorer.png')
        page.reload()
        expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
        check(theme_snapshot(page)['theme'] == 'observatory', 'theme choice survives a reload')
        page.get_by_role('button', name='Woven atlas').click()
        check(theme_snapshot(page)['theme'] == 'woven-atlas', 'back to woven atlas')

        # --- accessibility basics, motion, narrow screens ---------------------------------------
        page.reload()
        expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
        page.keyboard.press('Tab')
        check(page.evaluate('document.activeElement.classList.contains("skip")'), 'first Tab stop is the skip link')
        page.emulate_media(reduced_motion='reduce')
        check(page.evaluate('getComputedStyle(document.documentElement).getPropertyValue("--dur").trim()') in ('0ms', '0s'), 'reduced motion zeroes the motion duration')
        page.emulate_media(reduced_motion='no-preference')
        page.set_viewport_size({'width': 375, 'height': 800})
        page.wait_for_timeout(200)
        check(page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'no horizontal page scroll at 375 px')
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Simulation')
        shot('narrow.png')
        page.set_viewport_size({'width': 1360, 'height': 900})

        # --- no engine: unavailable, never demo data ------------------------------------------------
        page.evaluate('sessionStorage.clear()')
        page.goto(f'{base}/')
        alert = page.get_by_role('alert')
        expect(alert).to_contain_text('Unavailable')
        status = page.get_by_role('status', name='Session status')
        expect(status).to_contain_text('Dataset unknown')
        expect(status).to_contain_text('Simulation')
        expect(status).to_contain_text('Unavailable')
        check(page.locator('main table').count() == 0 and 'Synthetic' not in page.locator('main').inner_text(), 'no data and no demo data without an engine')
        shot('unavailable.png')

        # --- http mode against the fake engine ---------------------------------------------------
        page.goto(f'{base}/?transport=http#token={TOKEN}')  # an unknown transport value must not select mock
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
        check('token' not in page.url, 'the token is scrubbed from the address bar')
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Dataset personal')
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Browser')
        check(TOKEN in engine.tokens_seen and 'session.hello' in engine.calls, 'calls and the event stream carry the token header')
        check('Mock' not in page.get_by_role('status', name='Session status').inner_text(), 'http mode is not labelled mock')
        shot('http-connected.png')
        page.reload()
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
        print('PASS a reload of the tab keeps the session without the fragment')

        engine.down = True
        expect(page.get_by_role('alert')).to_contain_text('Unavailable', timeout=10000)
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Dataset unknown')
        check(page.locator('main h1', has_text='Explorer').count() == 0, 'the view is gone while the engine is down')
        engine.down = False
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected', timeout=20000)
        expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
        print('PASS drop to unavailable and recover')

        # --- product views (lane L13): Explorer, Tiers, Companion, Grants & health ------------------
        run_views(page, base, a.view_shots)

        # --- hygiene -------------------------------------------------------------------------------
        check(hosts <= {'127.0.0.1'}, f'every request stayed on loopback: {sorted(hosts)}')
        check(not errors, f'no console errors or page errors: {errors}')
        check(not dialogs, 'no dialog ever opened')
        browser.close()
    server.shutdown()
    print('ALL PASS')


if __name__ == '__main__':
    main()
