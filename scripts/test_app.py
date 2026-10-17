"""Browser checks for the Svelte app in app/ (lane L3). Requires Playwright and a Chromium installation.

Builds development fixtures under .loomward/app-e2e, serves them from a tiny loopback static
server and drives them with Chromium (production app/dist never enables the mock override):
  - mock mode (?transport=mock): shell chrome, labels, both themes, hostile names as text, routes;
  - no engine: "unavailable", never demo data;
  - http mode against a small in-process fake engine: token header, fragment scrubbed, SSE held open,
    and a dropped engine flips the shell to "unavailable" and back.
Run `npm.cmd --prefix app run build` first.

`--a11y` runs only the WCAG 2.2 AA leg (scripts/test_app_a11y.py, lane L18): names, focus, announcements,
zoom reflow, control-edge contrast, canvas keyboard and forced-colours states, on the mock transport.

`--live-serve` rebuilds app/dist first (the same build script, production mode), then replaces the Python
servers with the real `loomward-serve --static app/dist` binary (lane L6) over the engine service (lane L8) on a fresh synthetic state directory: the static files
under the server's own CSP header, the fragment-token handshake, calls and the SSE stream through
the real security boundary, and "unavailable" once the process is gone. A personal session granted one
small folder on the command line scans it when Scan is pressed, and its totals match the fixture.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import sqlite3
from contextlib import closing
import subprocess
import tempfile
from types import SimpleNamespace
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import Page, expect, sync_playwright

from test_app_collections import run_collections, run_live_collections
from test_app_a11y import run_a11y
import test_app_views
from test_app_views import run_views, wait_until
from test_app_copy import run_copy
from test_app_inventory import run_inventory
from test_app_ask import live_ask_isolated, run_ask

ROOT = Path(__file__).resolve().parents[1]
TOKEN = 'a1b2c3d4e5f60718293a4b5c6d7e8f90'
EPOCH = 'e_fake_epoch'
HOSTILE = '<img src=x onerror=alert(1)>.png'
MIME = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.json': 'application/json'}


class Engine:
    """Flags the test flips; a stand-in for loomward-serve answering the few commands the shell needs."""

    def __init__(self) -> None:
        self.down = False
        self.tokens_seen: set[str] = set()
        self.dataset = 'personal'
        self.calls: list[str] = []
        self.resume_ids: list[str] = []  # Last-Event-ID of every event-stream request


def envelope(req: dict, result=None, error=None) -> dict:
    base = {'protocol': 'loomward/3', 'request_id': req['request_id']}
    if error:
        return {**base, 'ok': False, 'error': {'code': error, 'message': f"fake engine: {req['command']} not implemented", 'retryable': False, 'detail': None}}
    return {**base, 'ok': True, 'result': result, 'meta': {'served_at': '2026-10-01T00:00:00Z', 'elapsed_ms': 0, 'dataset_class': engine.dataset, 'budget_hit': False, 'catalog_rev': None, 'state_rev': None}}


def fake_result(command: str):
    if command == 'session.hello':
        effects = {k: False for k in 'file_move file_delete file_rename file_write content_read process_kill process_suspend process_priority memory_trim uninstall elevation'.split()}
        return {
            'protocol': 'loomward/3', 'engine_version': 'fake-engine', 'adapter': 'http', 'dataset_class': engine.dataset,
            'session_started_at': '2026-10-01T00:00:00Z', 'enumeration_strategy': 'std_read_dir',
            'capabilities': {'observation': {'duplicate_content_read': False}, 'effects': effects},
            'features': {k: False for k in ('grant_picker', 'disclosure_dialog', 'teacher_available', 'telemetry_available', 'gpu_available', 'diagnostics_save', 'federated_search')},
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
            if 'Last-Event-ID' in self.headers:
                engine.resume_ids.append(self.headers['Last-Event-ID'])
            if engine.down:
                return self._send(503, b'{}', 'application/json')
            if not self._authorised():
                return
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Cache-Control', 'no-store')
            self.end_headers()
            try:
                hello = {'protocol': 'loomward/3', 'epoch': EPOCH, 'seq': 0, 'event': 'stream.hello', 'at': '2026-10-01T00:00:00Z', 'catalog_rev': None, 'state_rev': None,
                         'data': {'session_started_at': '2026-10-01T00:00:00Z', 'epoch': EPOCH, 'last_seq': 0, 'oldest_replayable_seq': 1, 'dataset_class': engine.dataset}}
                self.wfile.write(f'id: {EPOCH}.0\nevent: stream.hello\ndata: '.encode() + json.dumps(hello).encode() + b'\n\n')
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


def check_resume(page: Page) -> None:
    wait_until(page, lambda: f'{EPOCH}.0' in engine.resume_ids)
    check(all(re.fullmatch(rf'{EPOCH}\.\d+', i) for i in engine.resume_ids), f'the event stream resumes with the epoch-qualified id: {engine.resume_ids[:2]}')


def theme_snapshot(page: Page) -> dict:
    return page.evaluate("""() => {
      const cs = getComputedStyle(document.documentElement);
      return { theme: document.documentElement.dataset.theme, bg: cs.backgroundColor,
               num: getComputedStyle(document.querySelector('.chip strong')).fontFamily,
               meaning: cs.getPropertyValue('--meaning').trim(), residency: cs.getPropertyValue('--residency').trim(),
               permission: cs.getPropertyValue('--permission').trim() };
    }""")


def live_call(page: Page, token: str, command: str, payload: dict) -> dict:
    return page.evaluate("""async ([token, command, payload]) => (await (await fetch('/api/v3/call', {method: 'POST',
        headers: {'Content-Type': 'application/json', 'X-Loomward-Token': token},
        body: JSON.stringify({protocol: 'loomward/3', request_id: 'live_' + crypto.randomUUID().replaceAll('-', ''), command, payload})})).json())""", [token, command, payload])


def meaning_weft(page: Page, base: str) -> None:
    page.goto(f'{base}/?transport=mock#/atlas')
    expect(page.get_by_role('heading', name='Atlas', level=1)).to_be_visible()
    toggle = page.get_by_role('checkbox', name='Meaning', exact=True)
    expect(toggle).to_be_checked()
    expect(page.locator('main .legend')).to_contain_text('Suggestions, relative scores, not probabilities, never actions.')
    # The undyed states are told apart on the canvas itself (swatches share one renderer with the cloth), and the legend says what each means.
    expect(page.locator('main .legend')).to_contain_text('with no model, nothing has looked')
    swatch = page.evaluate("""() => Object.fromEntries([...document.querySelectorAll('main .legend li')].map(li => [li.textContent.trim(), li.querySelector('canvas').toDataURL()]))""")
    undyed = ['None', 'Pending', 'No model', 'Abstained', 'Not eligible']
    check(all(k in swatch for k in undyed) and len({swatch[k] for k in undyed}) == len(undyed), 'meaning: none, pending, no model, abstained and not eligible draw five different swatches')
    page.get_by_text('Meaning as text', exact=False).click()
    expect(page.locator('[aria-label="Visible meaning threads"] [data-meaning-state="abstained"]').first).to_contain_text('Abstained: neutral')
    neutral = page.locator('[aria-label="Visible meaning threads"] [data-meaning-state="abstained"]').first
    expect(neutral).to_contain_text('unfamiliar features')
    expect(neutral).to_contain_text('relative score')
    for theme_name in ['Observatory', 'Woven atlas']:
        page.get_by_role('button', name=theme_name, exact=True).click()
        expect(neutral).to_be_visible()
    page.emulate_media(forced_colors='active')
    expect(neutral).to_be_visible()
    toggle.uncheck()
    check(page.locator('[aria-label="Visible meaning threads"]').count() == 0, 'meaning: toggle hides the weft text projection')
    toggle.check()
    page.get_by_text('Meaning as text', exact=False).click()
    expect(page.locator('[aria-label="Visible meaning threads"] [data-meaning-state="suggested"]').first).to_contain_text('relative score')
    page.emulate_media(forced_colors='none')
    shots = ROOT / '.loomward'
    shots.mkdir(exist_ok=True)
    page.screenshot(path=str(shots / 'meaning-weft-desktop.png'), full_page=False)
    page.set_viewport_size({'width':390,'height':844})
    check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), 'meaning: narrow viewport has no page overflow')
    page.screenshot(path=str(shots / 'meaning-weft-mobile.png'), full_page=False)
    page.set_viewport_size({'width':1360,'height':900})
    page.evaluate("location.hash = '#/explorer'")
    expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
    page.get_by_role('button', name=re.compile('^Inspect ')).first.click()
    expect(page.get_by_role('region', name='Node inspector')).to_contain_text('Meaning thread')
    expect(page.get_by_role('region', name='Node inspector')).to_contain_text('Suggestions, relative scores, not probabilities, never actions.')


def live_learning_isolated(browser, exe: Path) -> None:
    """The learning leg plants SQL fixtures, as the collections leg does, so it runs on its own server and state."""
    state = tempfile.TemporaryDirectory(prefix='lw-live-learning-')
    proc = subprocess.Popen([str(exe), '--state-dir', state.name, '--static', str(DIST)], stdout=subprocess.PIPE, text=True)
    try:
        url = (proc.stdout.readline() if proc.stdout else '').strip()
        check('/#token=' in url, 'a synthetic loomward-serve is up for the learning leg')
        _, token = url.split('/#token=')
        page = browser.new_context(viewport={'width': 1360, 'height': 900}).new_page()
        page.set_default_timeout(8000)
        page.goto(url)
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
        live_learning(page, token, Path(state.name))
        page.context.close()
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=10)
        state.cleanup()


def live_learning(page: Page, token: str, state: Path) -> None:
    """Synthetic SQL fixture, no disk scan or native identity open. Real HTTP/service/student/UI."""
    with closing(sqlite3.connect(state / 'state.db')) as db, db:
        db.execute("INSERT INTO root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(1,'learning_fixture',zeroblob(16),'Synthetic learning root','fixture','fixture','active',0)")
    with closing(sqlite3.connect(state / 'catalog.db')) as db, db:
        db.executescript("""
            INSERT INTO volume(id,volume_key,display_name,filesystem,identity_json,capabilities_json,device_json,online,observed_at_ns)
                VALUES(1,'learning_fixture','Synthetic learning','NTFS','{}','{}','{}',1,0);
            INSERT INTO root(id,grant_id,volume_id,generation,state) VALUES(1,1,1,1,'complete');
            INSERT INTO scan_run(id,root_id,mode,strategy,state,started_at_ns,finished_at_ns,counters_json,budget_json)
                VALUES(1,1,'full','fixture','completed',0,0,'{}','{}');
            INSERT INTO dir(id,root_id,name,file_id,id_quality,id_basis,depth,attrs,created_ft,listing_state,born_run,seen_run,sub_complete)
                VALUES(1,1,'Synthetic learning',zeroblob(16),'native_file_id_128','listed',0,0,1,'complete',1,1,1);
            INSERT INTO file(id,dir_id,name,file_id,logical,allocated,created_ft,attrs,born_run,seen_run)
                VALUES(1,1,'synthetic-report.txt',cast('1111111111111111' AS BLOB),128,128,1,0,1,1);
            UPDATE revision SET catalog_rev=catalog_rev+1;
        """)
    queue = live_call(page, token, 'learning.queue', {'root_id': None, 'limit': 20, 'cursor': None})
    check(queue['ok'] and len(queue['result']['items']) == 1, 'learning: real service returns the synthetic durable node')
    node = queue['result']['items'][0]['row']['node_id']
    before_meaning = live_call(page, token, 'threads.meaning', {'node_ids':[node]})
    check(before_meaning['ok'] and before_meaning['result']['items'][0]['meaning']['state'] == 'no_model', 'meaning: real service is honest before fitting')
    payload = {'node_id': node, 'label': 'Documents', 'retract': False, 'client_event_id': 'ce_live_learning'}
    first = live_call(page, token, 'feedback.record', payload)
    check(first['ok'] and first['result']['data_class'] == 'human', 'learning: human feedback recorded through real HTTP')
    replay = live_call(page, token, 'feedback.record', payload)
    check(replay['ok'] and replay['result']['idempotent_replay'] and replay['result']['event_id'] == first['result']['event_id'], 'learning: durable client_event_id replay is idempotent')
    conflict = live_call(page, token, 'feedback.record', {**payload, 'label': 'Archive'})
    check(not conflict['ok'] and conflict['error']['code'] == 'invalid_request', 'learning: changed replay payload is refused')
    status = live_call(page, token, 'learning.status', {})
    check(status['ok'] and status['result']['human_events'] == 1 and status['result']['model_id'] is None, 'learning: one human event and honest unfitted status')
    page.evaluate("location.hash = '#/review'")
    expect(page.get_by_role('heading', name='Learning review', level=1)).to_be_visible()
    expect(page.get_by_text('No fitted model yet.', exact=False)).to_be_visible()
    expect(page.get_by_role('table')).to_contain_text('synthetic-report.txt')
    page.get_by_role('button', name='Refit from human labels').click()
    expect(page.get_by_role('status').filter(has_text='Refit completed.')).to_be_visible(timeout=10000)
    expect(page.get_by_text('Synthetic candidate fitted;', exact=False)).to_be_visible()
    status = live_call(page, token, 'learning.status', {})
    check(status['ok'] and status['result']['training_count'] == 1 and not status['result']['calibrated'] and not status['result']['autonomy_allowed'], 'learning: refit completed; synthetic smoke, not real-user accuracy')
    thread = live_call(page, token, 'threads.meaning', {'node_ids':[node]})
    check(thread['ok'] and thread['result']['model_id'] == status['result']['model_id'] and thread['result']['items'][0]['meaning']['suggestion']['abstain'] and thread['result']['items'][0]['meaning']['suggestion']['ranked'][0]['label'] == 'Documents', 'meaning: feedback and refit yield real ranked advice with honest abstention')
    page.get_by_role('button', name='Review synthetic-report.txt').focus()
    page.keyboard.press('Enter')
    page.get_by_label('Human label').select_option('Archive')
    page.get_by_role('button', name='Record human label').press('Space')
    expect(page.get_by_role('status').filter(has_text='Human feedback recorded, revision 2.')).to_be_visible()
    check(live_call(page, token, 'learning.status', {})['result']['human_events'] == 2, 'learning: review UI records real human feedback')
    expect(page.get_by_text('No fitted model yet.', exact=False)).to_be_visible()
    page.get_by_role('button', name='Review synthetic-report.txt').click()
    page.get_by_role('button', name='Withdraw human label').click()
    expect(page.get_by_role('status').filter(has_text='Human feedback recorded, revision 3.')).to_be_visible()
    expect(page.get_by_text('No fitted model yet.', exact=False)).to_be_visible()
    status = live_call(page, token, 'learning.status', {})
    check(status['ok'] and status['result']['model_id'] is None and status['result']['human_support']['Documents'] == 0,
          'learning: newer feedback and withdrawal retire candidate advice immediately')
    def unavailable_status(route):
        request = route.request.post_data_json
        if request['command'] == 'learning.status':
            route.fulfill(json={'protocol': 'loomward/3', 'request_id': request['request_id'], 'ok': False,
                                'error': {'code': 'capability_unavailable', 'message': 'Learning status unavailable.', 'retryable': False, 'detail': None}})
        else:
            route.continue_()
    page.route('**/api/v3/call', unavailable_status)
    page.get_by_role('button', name='Refresh review').click()
    expect(page.get_by_text('Learning status unknown.', exact=True)).to_be_visible()
    check(page.get_by_role('table').count() == 0, 'learning: a failed refresh clears stale learning data')
    page.unroute('**/api/v3/call', unavailable_status)
    page.get_by_role('button', name='Retry learning status').click()
    expect(page.get_by_role('table')).to_contain_text('synthetic-report.txt')
    shots = ROOT / '.loomward'
    shots.mkdir(exist_ok=True)
    page.screenshot(path=str(shots / 'learning-review-desktop.png'), full_page=True)
    page.set_viewport_size({'width': 390, 'height': 844})
    page.screenshot(path=str(shots / 'learning-review-mobile.png'), full_page=True)
    check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), 'learning: mobile review has no page overflow')
    page.set_viewport_size({'width': 1360, 'height': 900})


def live_placement(browser, exe: Path) -> None:
    """A tiny synthetic catalogue through the production service and app; no host inventory."""
    with tempfile.TemporaryDirectory(prefix='lw-live-placement-') as state:
        proc = subprocess.Popen([str(exe), '--state-dir', state, '--static', str(DIST)], stdout=subprocess.PIPE, text=True)
        try:
            url = (proc.stdout.readline() if proc.stdout else '').strip()
            check('/#token=' in url, 'a synthetic placement service is running')
            _, token = url.split('/#token=')
            with sqlite3.connect(Path(state) / 'state.db') as db:
                db.execute("INSERT INTO root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(1,'synthetic-source',?,'Synthetic fixture','fixture','fixture','active',0)", [bytes([1] * 16)])
                db.execute('UPDATE revision SET state_rev=state_rev+1')
            db.close()
            with sqlite3.connect(Path(state) / 'catalog.db') as db:
                physics = json.dumps({'space': {'capacity_bytes': '1048576', 'free_bytes': '65536', 'read_only': False}})
                db.execute("INSERT INTO volume(id,volume_key,display_name,bytes_per_cluster,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES(1,'synthetic-source','Synthetic source',4096,'{}',?,'{}',1,0)", [physics])
                db.execute("INSERT INTO volume(id,volume_key,display_name,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES(2,'synthetic-unknown','Synthetic unknown','{}','{}','{}',1,0)")
                db.execute("INSERT INTO root(id,grant_id,volume_id,root_file_id,generation,state) VALUES(1,1,1,?,1,'complete')", [bytes([1] * 16)])
                db.execute("INSERT INTO dir(id,root_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,sub_files,sub_logical,sub_allocated,sub_complete) VALUES(1,1,'Synthetic group',?,'native_file_id_128','listed',0,16,'complete',1,1,1,4096,4096,1)", [bytes([1] * 16)])
                db.execute("INSERT INTO file(id,dir_id,name,file_id,logical,allocated,attrs,born_run,seen_run) VALUES(1,1,'synthetic.bin',?,4096,4096,0,1,1)", [bytes([2] * 16)])
                db.execute('UPDATE revision SET catalog_rev=1')
            db.close()
            page = browser.new_context(viewport={'width': 1360, 'height': 900}).new_page()
            errors: list[str] = []
            page.on('pageerror', lambda e: errors.append(str(e)))
            page.goto(url)
            expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
            page.evaluate("location.hash = '#/tiers'")
            expect(page.get_by_role('heading', name='Tiers', level=1)).to_be_visible()
            page.get_by_label('Declared tier for Synthetic source').select_option('0')
            expect(page.get_by_role('button', name=re.compile(r'Source volume\s*:\s*Synthetic source'))).to_be_visible()
            expect(page.locator('#h-cand + p')).to_contain_text('Default-stream')
            expect(page.locator('main table.tbl').first.locator('tbody')).to_contain_text('Synthetic group')
            expect(page.locator('main table.tbl').first.locator('tbody')).to_contain_text('unknown')
            page.get_by_label('Keep these scenarios').check()
            page.get_by_role('button', name='Simulate', exact=True).click()
            expect(page.get_by_role('heading', name='Placement plan', exact=True)).to_be_visible()
            expect(page.get_by_text('Space relief cannot be verified, so it is unknown rather than zero')).to_be_visible()
            expect(page.get_by_text('Synthetic unknown', exact=True).last).to_be_visible()
            expect(page.get_by_role('button', name=re.compile('Show saved scenario'))).to_have_count(3)
            page.get_by_role('button', name=re.compile('Show saved scenario')).first.click()
            expect(page.locator('main table.alts')).to_have_count(0)
            expect(page.get_by_role('heading', name='Placement plan', exact=True)).to_be_visible()
            proposals = live_call(page, token, 'proposals.list', {})
            check(proposals['ok'] and len(proposals['result']['proposals']) == 3, 'the real service saved all three simulation alternatives')
            detail = live_call(page, token, 'proposals.get', {'proposal_id': proposals['result']['proposals'][0]['proposal_id']})
            check(detail['ok'] and detail['result']['plan']['filesystem_changed'] is False and detail['result']['summary']['status'] == 'simulation_only', 'saved scenario retrieval remains simulation only')
            check(not errors, 'the real Tiers interaction has no page errors')
            page.context.close()
        finally:
            if proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=10)


def live_scan_refusals(browser, exe: Path) -> None:
    """A synthetic session with no root grants offers no Scan button and refuses scan.start for a root it does not
    know, without starting a job."""
    state = tempfile.TemporaryDirectory(prefix='lw-live-scan-synthetic-')
    proc = subprocess.Popen([str(exe), '--state-dir', state.name, '--static', str(DIST)], stdout=subprocess.PIPE, text=True)
    try:
        url = (proc.stdout.readline() if proc.stdout else '').strip()
        check('/#token=' in url, 'a synthetic loomward-serve is up for the scan refusal checks')
        _, token = url.split('/#token=')
        page = browser.new_context(viewport={'width': 1360, 'height': 900}).new_page()
        page.set_default_timeout(8000)
        page.goto(url)
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
        page.evaluate("location.hash = '#/duplicates'")
        expect(page.get_by_role('heading', name='Duplicates', level=1)).to_be_visible()
        expect(page.get_by_role('button', name='Start inspection')).to_be_disabled()
        inspected = live_call(page, token, 'duplicates.inspect', {'root_id': 'rt_nope', 'budget': {'max_candidates': 200, 'max_bytes': '1024', 'max_ms': 1000}})
        check(not inspected['ok'] and inspected['error']['code'] == 'not_found', 'synthetic: inspection requires a real registered root')
        page.evaluate("location.hash = '#/health'")
        expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
        expect(page.locator('main table.tbl tbody')).to_contain_text('No roots granted')
        check(page.locator('ul.scans li').count() == 0 and page.locator('ul.scans button').count() == 0, 'synthetic: no root, so no Scan control is offered')
        check(page.locator('p.bad.live').inner_text() == '', 'synthetic: Grants & health loaded against the real service (jobs.list answered), no error shown')
        check(page.get_by_role('button', name='Add a folder…').count() == 0 and 'Folders are added in the desktop app.' in page.locator('main').inner_text(), 'synthetic: loomward-serve has no folder picker, so no Add a folder button, and the page says where folders are added')
        r = live_call(page, token, 'scan.start', {'root_id': 'rt_nope', 'mode': 'full'})
        check(not r['ok'] and r['error']['code'] == 'not_found', f"synthetic: scan.start for an unknown root is not_found: {r.get('error')}")
        jobs = live_call(page, token, 'jobs.list', {'kinds': ['scan'], 'limit': 50})
        check(jobs['ok'] and jobs['result']['jobs'] == [], 'synthetic: the refused scan.start created no job')
        page.context.close()
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=10)
        state.cleanup()


# A small owner folder: (relative path, bytes), every byte 0x01. Five files and three folders below the root, 145 bytes;
# a/1.bin and b/1-copy.bin are the one synthetic duplicate pair (the only two files of one size).
PERSONAL_FIXTURE = [('top.bin', 100), ('a/1.bin', 10), ('a/2.bin', 20), ('a/deep/x.bin', 5), ('b/1-copy.bin', 10)]


def live_personal_scan(browser, exe: Path) -> None:
    """Personal scans are open: a folder granted through the server's command line (the service's real grant path)
    is scanned by pressing Scan in the app, completes, and its totals match the fixture. An unknown root is refused.
    Personal duplicate reads are open (q-10): pressing Start inspection confirms the fixture's one duplicate pair,
    changes no file, and revoking the root redacts the report.

    The folder lives under target/test-roots, not the system temp folder: a profile's AppData is never eligible.
    On an elevated host the grant is refused (identity_unavailable) and the server does not start; that is checked."""
    roots = ROOT / 'target' / 'test-roots'
    roots.mkdir(parents=True, exist_ok=True)
    fixture = tempfile.TemporaryDirectory(prefix='lw-live-personal-', dir=roots)
    folder = Path(fixture.name) / 'owner-folder'
    (folder / 'b').mkdir(parents=True)
    for rel, size in PERSONAL_FIXTURE:
        (folder / rel).parent.mkdir(parents=True, exist_ok=True)
        (folder / rel).write_bytes(b'\x01' * size)
    snapshot = lambda: {rel: ((folder / rel).read_bytes(), (folder / rel).stat().st_mtime_ns) for rel, _ in PERSONAL_FIXTURE}
    before = snapshot()
    state = tempfile.TemporaryDirectory(prefix='lw-live-scan-personal-')
    proc = subprocess.Popen([str(exe), '--state-dir', state.name, '--static', str(DIST), '--dataset', 'personal', '--allow-personal', '--grant-root', str(folder)],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        url = (proc.stdout.readline() if proc.stdout else '').strip()
        if not url:
            err = proc.communicate(timeout=10)[1]
            check('identity_unavailable' in err, f'personal: the server refused the grant only because this host is elevated: {err.strip()}')
            print('PASS personal: an elevated host refuses the grant (identity_unavailable); the scan leg needs a non-elevated host')
            return
        check('/#token=' in url, 'a personal loomward-serve is up with one granted folder')
        _, token = url.split('/#token=')
        page = browser.new_context(viewport={'width': 1360, 'height': 900}).new_page()
        page.set_default_timeout(8000)
        page.goto(url)
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Dataset personal')
        page.evaluate("location.hash = '#/health'")
        expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
        item = page.locator('ul.scans li')
        expect(item).to_have_count(1)
        expect(item).to_contain_text('owner granted')
        check(page.locator('p.bad.live').inner_text() == '', 'personal: Grants & health loaded with no error shown')
        item.get_by_role('button', name=re.compile(r'^Scan .*owner-folder$')).click()
        expect(item.locator('.job')).to_contain_text('Finished, complete coverage', timeout=30000)
        listed = live_call(page, token, 'roots.list', {})
        root = listed['result']['roots'][0]
        totals = root.get('totals') or {}
        check(root['origin'] == 'owner_granted' and root['scan']['coverage'] == 'complete', f"personal: the granted folder is scanned with complete coverage: {root['scan']}")
        check(totals.get('files') == 5 and str(totals.get('logical_bytes')) == '145', f'personal: totals match the fixture (5 files, 145 bytes): {totals}')
        check(snapshot() == before, 'personal: the scan changed no file (contents and modification times are identical)')
        r = live_call(page, token, 'scan.start', {'root_id': 'rt_nope', 'mode': 'full'})
        check(not r['ok'] and r['error']['code'] == 'not_found', f"personal: scan.start for an unknown root is not_found: {r.get('error')}")
        # Personal duplicate reads are open (q-10): the view says what it reads, and the granted folder's pair is confirmed.
        page.evaluate("location.hash = '#/duplicates'")
        expect(page.get_by_role('heading', name='Duplicates', level=1)).to_be_visible()
        expect(page.get_by_text('Reads file contents on this machine to confirm duplicates; nothing is deleted or moved.', exact=True)).to_be_visible()
        page.get_by_role('button', name='Start inspection').click()
        expect(page.get_by_role('rowheader', name=re.compile(r'1\.bin.*1-copy\.bin|1-copy\.bin.*1\.bin'))).to_be_visible(timeout=15000)
        dup_jobs = live_call(page, token, 'jobs.list', {'kinds': ['duplicates'], 'limit': 50})['result']['jobs']
        check(len(dup_jobs) == 1 and dup_jobs[0]['state'] == 'completed', f'personal: one duplicates job completed: {dup_jobs}')
        report = dup_jobs[0]['duplicates']
        names = sorted(report['entries'][i]['relative_path'].replace(chr(92), '/') for i in report['groups'][0]) if len(report['groups']) == 1 else []
        check(names == ['a/1.bin', 'b/1-copy.bin'], f'personal: the one synthetic duplicate pair is confirmed: {report}')
        check(not report['skipped'] and report['stop'] is None and 0 < int(report['bytes_read']) <= 60, f'personal: content was read within the stages, nothing skipped: {report}')
        check(snapshot() == before, 'personal: inspection read the files and changed none of them')
        inspected = live_call(page, token, 'duplicates.inspect', {'root_id': 'rt_nope', 'budget': {'max_candidates': 200, 'max_bytes': '1024', 'max_ms': 1000}})
        check(not inspected['ok'] and inspected['error']['code'] == 'not_found', 'personal: inspection of an unknown root is not_found')
        jobs = live_call(page, token, 'jobs.list', {'kinds': ['scan'], 'limit': 50})
        check(jobs['ok'] and len(jobs['result']['jobs']) == 1, f"personal: only the pressed Scan created a job: {jobs.get('result')}")
        # q-11: the Review surface is open for personal sessions, and the teacher stays refused.
        page.evaluate("location.hash = '#/review'")
        expect(page.get_by_text('Your labels train a local student on this machine.', exact=False)).to_be_visible()
        expect(page.get_by_role('button', name='Refit from human labels')).to_be_enabled()
        teacher_refusals(page, token, 'personal')
        # Forget (owner decision q-9): revoke in the app, then forget the revoked root. The folder's name leaves every
        # file of the state directory (databases, WALs and the start-up backups), a dated record stays, no user file moves.
        def holders(needle: str) -> list[str]:
            # state.db, its WAL and the start-up backups: what precious state keeps. catalog.db is derived and may hold
            # freed pages from before the forget (see the PR), so it is checked through the API instead.
            files = [p for p in Path(state.name).glob('state*') if p.is_file()]
            return [p.name for p in files if needle.encode() in p.read_bytes() or needle.encode('utf-16-le') in p.read_bytes()]
        stored = lambda needle: bool(holders(needle))
        check(stored('owner-folder'), 'forget: the granted folder is named in state.db or its WAL before it is forgotten')
        root_id = live_call(page, token, 'roots.list', {})['result']['roots'][0]['root_id']
        active = live_call(page, token, 'roots.forget', {'root_id': root_id})
        check(not active['ok'] and active['error']['code'] == 'invalid_request' and active['error']['detail']['reason'] == 'root_active', f"forget: an active root is refused with a reason: {active.get('error')}")
        page.evaluate("location.hash = '#/health'")
        expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
        row = page.locator('main section[aria-labelledby="h-roots"] table tbody tr').first
        row.get_by_role('button', name=re.compile('^Revoke…')).click()
        page.get_by_role('group', name=re.compile('Revoke root access')).get_by_role('button', name='Revoke', exact=True).click()
        expect(row.get_by_role('button', name=re.compile('^Forget…'))).to_be_visible()
        # Revoking the root redacts its duplicates report (#318): the job outcome stays, its names do not.
        after = live_call(page, token, 'jobs.get', {'job_id': dup_jobs[0]['job_id']})['result']['job']
        check(after['state'] == 'completed' and after.get('duplicates') is None, f"personal: a revoked root's report is redacted: {after}")
        row.get_by_role('button', name=re.compile('^Forget…')).click()
        ask = page.get_by_role('group', name='Forget owner-folder?')
        expect(ask).to_contain_text('Your files are not touched')
        ask.get_by_role('button', name='Forget', exact=True).click()
        expect(page.get_by_role('status').filter(has_text='Forgotten')).to_contain_text('No scanned file was touched')
        expect(page.locator('main section[aria-labelledby="h-roots"] table tbody')).to_contain_text('No roots granted')
        grants = live_call(page, token, 'grants.list', {})['result']['grants']
        record = [g for g in grants if g['kind'] == 'metadata_root']
        check(len(record) == 1 and record[0]['revoked_at'] and record[0]['forgotten_at'] and 'owner-folder' not in json.dumps(grants), f'forget: grants.list keeps a dated record without the name: {record}')
        check(live_call(page, token, 'roots.list', {})['result']['roots'] == [], 'forget: roots.list no longer lists the root')
        gone = live_call(page, token, 'scan.start', {'root_id': root_id, 'mode': 'full'})
        check(not gone['ok'] and gone['error']['code'] == 'not_found', f"forget: the forgotten root can no longer be scanned: {gone.get('error')}")
        page.context.close()  # no page request can still be reading while the log is cut
        until = time.time() + 15  # the WAL is cut as soon as no reader holds an older snapshot
        while stored('owner-folder') and time.time() < until:
            time.sleep(0.25)
        check(not stored('owner-folder'), f"forget: the folder name is in no state.db, WAL or start-up backup: {holders('owner-folder')}")
        check(snapshot() == before, 'forget: no file of the granted folder changed (contents and modification times are identical)')
        print('PASS personal: a granted folder scans to the fixture totals and its duplicate pair is confirmed; revocation redacts; forget removes the name')
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=10)
        state.cleanup()
        fixture.cleanup()


def teacher_refusals(page: Page, token: str, what: str, node: str = 'nd_0020') -> None:
    for command, payload in [('teacher.preview', {'node_ids': [node], 'recipient': 'codex_cli_gpt_6_1_sol'}),
                             ('grants.create_disclosure', {'preview_id': 'tp_0001', 'payload_digest': 'sha256:' + '0' * 64}),
                             ('teacher.run', {'grant_id': 'gr_0002'}), ('teacher.results', {'job_id': 'jb_0010'})]:
        refused = live_call(page, token, command, payload)
        check(not refused['ok'] and (refused['error']['detail'] or {}).get('reason') == 'teacher_personal_gated', f'{what}: {command} refuses with teacher_personal_gated')


PERSONAL_LEARNING_FILES = ['top.bin', '1.bin', '2.bin', 'x.bin']


def live_personal_learning_isolated(browser, exe: Path) -> None:
    """q-11 end to end on a disposable personal-session fixture root: SQL rows planted in a fresh personal state
    (origin owner_granted, synthetic names, no disk scan, no user path), driven through the real HTTP service and app."""
    state = tempfile.TemporaryDirectory(prefix='lw-live-personal-learning-')
    proc = subprocess.Popen([str(exe), '--state-dir', state.name, '--static', str(DIST), '--dataset', 'personal', '--allow-personal'],
                            stdout=subprocess.PIPE, text=True)
    try:
        url = (proc.stdout.readline() if proc.stdout else '').strip()
        check('/#token=' in url, 'a personal loomward-serve is up for the personal learning leg')
        _, token = url.split('/#token=')
        with closing(sqlite3.connect(Path(state.name) / 'state.db')) as db, db:
            db.execute("INSERT INTO root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(1,'learning_fixture',zeroblob(16),'Disposable personal fixture','owner_granted','cli_flag','active',0)")
        rows = ''.join(f"INSERT INTO file(id,dir_id,name,file_id,logical,allocated,created_ft,attrs,born_run,seen_run) VALUES({i},1,'{n}',cast('{i:016d}' AS BLOB),{10 * i},{10 * i},1,0,1,1);"
                       for i, n in enumerate(PERSONAL_LEARNING_FILES, 1))
        with closing(sqlite3.connect(Path(state.name) / 'catalog.db')) as db, db:
            db.executescript("""
                INSERT INTO volume(id,volume_key,display_name,filesystem,identity_json,capabilities_json,device_json,online,observed_at_ns)
                    VALUES(1,'learning_fixture','Personal fixture','NTFS','{}','{}','{}',1,0);
                INSERT INTO root(id,grant_id,volume_id,generation,state) VALUES(1,1,1,1,'complete');
                INSERT INTO scan_run(id,root_id,mode,strategy,state,started_at_ns,finished_at_ns,counters_json,budget_json)
                    VALUES(1,1,'full','fixture','completed',0,0,'{}','{}');
                INSERT INTO dir(id,root_id,name,file_id,id_quality,id_basis,depth,attrs,created_ft,listing_state,born_run,seen_run,sub_complete)
                    VALUES(1,1,'Personal fixture',zeroblob(16),'native_file_id_128','listed',0,0,1,'complete',1,1,1);
            """ + rows + "UPDATE revision SET catalog_rev=catalog_rev+1;")
        page = browser.new_context(viewport={'width': 1360, 'height': 900}).new_page()
        page.set_default_timeout(8000)
        page.goto(url)
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Dataset personal')
        live_personal_learning(page, token)
        page.context.close()
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=10)
        state.cleanup()


def live_personal_learning(page: Page, token: str) -> None:
    """q-11: labels made in the app train the local student; the teacher stays refused; revoking the root redacts,
    withdraws and blocks new labels while withdrawal stays possible."""
    page.evaluate("location.hash = '#/review'")
    expect(page.get_by_role('heading', name='Learning review', level=1)).to_be_visible()
    expect(page.get_by_text('Your labels train a local student on this machine.', exact=False)).to_be_visible()
    check(page.get_by_text('Learning is gated', exact=False).count() == 0, 'personal learning: the Review surface is not gated')
    status = live_call(page, token, 'learning.status', {})
    check(status['ok'] and status['result']['gate'] is None and status['result']['dataset_class'] == 'personal', f"personal learning: status is open: {status.get('result') or status.get('error')}")
    queue = live_call(page, token, 'learning.queue', {'root_id': None, 'limit': 20, 'cursor': None})
    items = {i['row']['name']: i['row']['node_id'] for i in (queue.get('result') or {}).get('items', [])}
    check(queue['ok'] and set(items) == set(PERSONAL_LEARNING_FILES), f'personal learning: the queue lists the granted root files: {sorted(items)}')
    page.get_by_role('button', name='Review top.bin').click()
    page.get_by_label('Human label').select_option('Archive')
    page.get_by_role('button', name='Record human label').click()
    expect(page.get_by_role('status').filter(has_text='Human feedback recorded, revision 1.')).to_be_visible()
    second = live_call(page, token, 'feedback.record', {'node_id': items['x.bin'], 'label': 'Media', 'retract': False, 'client_event_id': 'ce_personal_live_x'})
    check(second['ok'] and second['result']['data_class'] == 'human', 'personal learning: a second human label is recorded through HTTP')
    page.get_by_role('button', name='Refit from human labels').click()
    expect(page.get_by_role('status').filter(has_text='Refit completed.')).to_be_visible(timeout=10000)
    expect(page.get_by_text('Personal candidate fitted;', exact=False)).to_be_visible()
    status = live_call(page, token, 'learning.status', {})['result']
    check(status['training_count'] == 2 and status['teacher_labels'] == 0 and not status['calibrated'] and not status['autonomy_allowed'],
          f'personal learning: the refit trains on the two human labels only, uncalibrated, no autonomy: {status}')
    thread = live_call(page, token, 'threads.meaning', {'node_ids': [items['top.bin'], items['1.bin']]})
    first = (thread.get('result') or {}).get('items', [{}])[0].get('meaning', {})
    check(thread['ok'] and thread['result']['model_id'] == status['model_id'] and first.get('state') in ('suggested', 'abstained')
          and first['suggestion']['teacher'] is None and not first['suggestion']['calibrated'],
          f"personal learning: meaning threads read the local candidate: {thread.get('result') or thread.get('error')}")
    teacher_refusals(page, token, 'personal learning', items['top.bin'])
    root = live_call(page, token, 'roots.list', {})['result']['roots'][0]['root_id']
    check(live_call(page, token, 'roots.revoke', {'root_id': root, 'purge_catalog': False})['ok'], 'personal learning: the root is revoked')
    status = live_call(page, token, 'learning.status', {})['result']
    check(status['model_id'] is None, 'personal learning: revocation deletes the candidate fitted from that root')
    late = live_call(page, token, 'feedback.record', {'node_id': items['1.bin'], 'label': 'Archive', 'retract': False, 'client_event_id': 'ce_personal_live_late'})
    check(not late['ok'] and late['error']['code'] == 'not_found', f"personal learning: a revoked root's node cannot be labelled: {late.get('error')}")
    withdrawn = live_call(page, token, 'feedback.record', {'node_id': items['x.bin'], 'label': None, 'retract': True, 'client_event_id': 'ce_personal_live_withdraw'})
    check(withdrawn['ok'] and withdrawn['result']['revision'] == 2, f"personal learning: withdrawal stays possible after revocation: {withdrawn.get('error')}")
    refit = live_call(page, token, 'learning.refit', {})
    check(refit['ok'], f"personal learning: refit after revocation starts: {refit.get('error')}")
    job = refit['result']['job']
    for _ in range(200):
        job = live_call(page, token, 'jobs.get', {'job_id': job['job_id']})['result']['job']
        if job['finished_at']:
            break
        page.wait_for_timeout(50)
    status = live_call(page, token, 'learning.status', {})['result']
    check(job['state'] == 'completed' and status['training_count'] == 0 and status['model_id'] is None,
          f'personal learning: revocation withdraws every label of the root from training: {status}')


def build_app(out: Path | None, development: bool) -> None:
    """The app's `build` script, the one build both modes run: into app/dist when out is None, else into out."""
    args = ['npm.cmd' if os.name == 'nt' else 'npm', '--prefix', 'app', 'run', 'build']
    if out is not None:
        args += ['--', '--outDir', str(out)]
    subprocess.run(args, cwd=ROOT, env=dict(os.environ, NODE_ENV='development') if development else None, check=True)


def live_backups(page: Page, token: str) -> None:
    """Backups over the real HTTP adapter: read and forget exist, import does not (no Rust-owned dialog)."""
    listed = live_call(page, token, 'backups.list', {})
    check(listed['ok'] and listed['result'] == {'evidence': []} and listed['meta']['state_rev'] is not None, f'backups.list answers an empty synthetic list: {listed}')
    imported = live_call(page, token, 'backups.import', {'repository_label': 'synthetic-nas', 'snapshot_id': '1' * 64, 'source_volume_id': None, 'target_volume_id': None,
                                                          'required': [{'relative_name': 'fixture/ledger.ods', 'version': 'sha256:' + 'a' * 64}], 'integrity_check': False})
    check(not imported['ok'] and imported['error']['code'] == 'capability_unavailable', f'backups.import is not offered over HTTP: {imported}')
    forgot = live_call(page, token, 'backups.forget', {'evidence_id': 'be_1'})
    check(not forgot['ok'] and forgot['error']['code'] == 'not_found', f'backups.forget of an unknown record is not_found: {forgot}')
    page.evaluate("location.hash = '#/backups'")
    expect(page.get_by_role('heading', name='Backups', level=1)).to_be_visible()
    expect(page.get_by_text('No backup evidence imported yet.')).to_be_visible()
    expect(page.get_by_text('Importing needs the desktop app in a personal session')).to_be_visible()
    check(page.locator('main p.bad.live').inner_text() == '', 'backups: the view loaded from the engine with no error')
    print('PASS live Backups: list and forget over HTTP, import refused, the view shows no evidence rather than demo data')


def live_alerts(page: Page, token: str) -> None:
    """LW-121 alerts over the real HTTP adapter: defaults first, nothing claimed about an unobserved volume or a root with no history."""
    listed = live_call(page, token, 'alerts.list', {})
    check(listed['ok'] and listed['result']['settings'] == listed['result']['defaults'] and listed['result']['settings']['notifications_enabled'] is False,
          f'alerts.list answers the documented defaults with notifications off: {listed}')
    check(listed['result']['active_count'] == 0 and all(i['status'] != 'alert' for i in listed['result']['items']), 'no alert is raised from nothing')
    check(all(i['measured_bytes'] is None for i in listed['result']['items'] if i['status'] in ('unknown', 'insufficient_history')), 'unknown and missing history carry no measurement, never zero')
    settings = dict(listed['result']['settings'], low_space_free_percent=20)
    saved = live_call(page, token, 'alerts.configure', settings)
    check(saved['ok'] and saved['result']['settings']['low_space_free_percent'] == 20 and saved['meta']['state_rev'] is not None, f'alerts.configure stores the thresholds: {saved}')
    bad = live_call(page, token, 'alerts.configure', dict(settings, low_space_free_percent=0))
    check(not bad['ok'] and bad['error']['code'] == 'invalid_request', f'an out-of-range threshold is refused: {bad}')
    page.evaluate("location.hash = '#/alerts'")
    expect(page.get_by_role('heading', name='Alerts', level=1)).to_be_visible()
    expect(page.get_by_role('heading', name='Thresholds', level=2)).to_be_visible()
    expect(page.get_by_label('Low space: percent of capacity')).to_have_value('20')
    check(page.locator('main p.bad.live').inner_text() == '', 'alerts: the view loaded from the engine with no error')
    print('PASS live Alerts: defaults, unknowns suppressed, thresholds stored and refused out of range')


def live_diagnostics(page: Page, token: str) -> None:
    """The diagnostic bundle over the real HTTP adapter: previewed and checked, never saved (no Rust-owned dialog)."""
    previewed = live_call(page, token, 'diagnostics.preview', {})
    check(previewed['ok'], f'diagnostics.preview answers over HTTP: {previewed}')
    r = previewed['result']
    text = json.dumps(r['bundle'], indent=2, sort_keys=True, ensure_ascii=False).encode('utf-8')
    check(r['bytes'] == str(len(text)) and r['digest'] == 'sha256:' + hashlib.sha256(text).hexdigest(),
          'the advertised size and digest are those of the bundle\'s canonical text')
    check(r['bundle']['loomward']['adapter'] == 'http' and r['bundle']['session']['features']['diagnostics_save'] is False,
          'the HTTP bundle says it cannot save')
    saved = live_call(page, token, 'diagnostics.save', {'digest': r['digest']})
    check(not saved['ok'] and saved['error']['code'] == 'capability_unavailable', f'diagnostics.save is not offered over HTTP: {saved}')
    page.evaluate("location.hash = '#/health'")
    panel = page.locator('section[aria-labelledby="h-diag"]')
    expect(panel.get_by_text('the text below is exactly these bytes')).to_be_visible()
    expect(panel.get_by_text('Saving needs the desktop app')).to_be_visible()
    check(panel.get_by_role('button', name='Save…').count() == 0, 'no Save button over HTTP')
    print('PASS live diagnostic bundle: preview over HTTP matches its digest, save refused, the view offers no save')


def run_live_serve(browser_path: str | None) -> None:
    """The app's http transport against the real loomward-serve binary (lane L6) and engine service (L8)."""
    # loomward-broker lands beside loomward-serve: the service spawns it per Ask round (LW-118).
    subprocess.run(['cargo', 'build', '-q', '-p', 'loomward-http', '--bin', 'loomward-serve'], cwd=ROOT, check=True)
    subprocess.run(['cargo', 'build', '-q', '-p', 'loomward-broker', '--bin', 'loomward-broker'], cwd=ROOT, check=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target')
    exe = target / 'debug' / ('loomward-serve.exe' if sys.platform == 'win32' else 'loomward-serve')
    state = tempfile.TemporaryDirectory(prefix='lw-live-serve-')
    proc = subprocess.Popen([str(exe), '--state-dir', state.name, '--static', str(DIST)], stdout=subprocess.PIPE, text=True)
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
            expect(page.get_by_role('heading', name='Unavailable', level=1)).to_be_visible()
            expect(page.get_by_role('status', name='Session status')).to_contain_text('No transport')
            print('PASS the production app refuses mock mode under the server CSP')

            page.evaluate('sessionStorage.clear()')
            page.goto(url)
            status = page.get_by_role('status', name='Session status')
            expect(status).to_contain_text('Connected')
            check('token' not in page.url, 'the token is scrubbed from the address bar')
            expect(status).to_contain_text('Dataset synthetic')
            expect(status).to_contain_text('Browser')
            check(page.evaluate(f"""async () => (await fetch('/api/v3/call', {{method: 'POST', body: '{{}}', headers: {{'Content-Type': 'application/json'}}}})).status""") == 403, 'a call without the token header is refused')
            hello = page.evaluate("""async (token) => (await (await fetch('/api/v3/call', {method: 'POST', headers: {'Content-Type': 'application/json', 'X-Loomward-Token': token},
                body: JSON.stringify({protocol: 'loomward/3', request_id: 'live_1', command: 'session.hello', payload: {}})})).json())""", token)
            check(hello['ok'] and hello['result']['engine_version'].startswith('loomward-service/'), 'the engine service answers, not the fixtures')
            roots = page.evaluate("""async (token) => (await (await fetch('/api/v3/call', {method: 'POST', headers: {'Content-Type': 'application/json', 'X-Loomward-Token': token},
                body: JSON.stringify({protocol: 'loomward/3', request_id: 'live_2', command: 'roots.list', payload: {}})})).json())""", token)
            check(roots['ok'] and roots['result']['roots'] == [] and roots['meta']['state_rev'] is not None, 'a fresh synthetic catalogue lists no roots, never demo data')
            page.wait_for_timeout(1500)  # the SSE stream stays open: still connected
            expect(status).to_contain_text('Connected')
            print('PASS http transport connected through loomward-serve')
            run_live_collections(page, token, Path(state.name), check)
            run_inventory(page, base, check, live=True)
            live_backups(page, token)
            live_alerts(page, token)
            live_diagnostics(page, token)
            live_scan_refusals(browser, exe)
            live_personal_scan(browser, exe)
            live_placement(browser, exe)
            live_learning_isolated(browser, exe)
            live_personal_learning_isolated(browser, exe)
            live_ask_isolated(browser, exe, DIST, check, live_call)

            proc.terminate()
            proc.wait(timeout=10)
            expect(page.get_by_role('alert').filter(has_text='Unavailable')).to_contain_text('Unavailable', timeout=15000)
            expect(status).to_contain_text('Dataset unknown')
            print('PASS a stopped server flips the shell to unavailable')

            check(hosts <= {'127.0.0.1'}, f'every request stayed on loopback: {sorted(hosts)}')
            unexpected = [e for e in errors if 'Failed to load resource' not in e and 'ERR_CONNECTION_REFUSED' not in e]
            check(not unexpected, f'no console errors or page errors: {unexpected}')
            browser.close()
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=10)
        state.cleanup()
    print('ALL PASS (live serve)')


def main() -> None:
    global DIST
    ap = argparse.ArgumentParser()
    ap.add_argument('--browser', default=None, help='path to a Chromium executable')
    ap.add_argument('--screenshots', type=Path)
    ap.add_argument('--view-shots', type=Path, help='directory for view screenshots and results from both view lanes (evidence/v3/app-views); omitted = none')
    ap.add_argument('--a11y', action='store_true', help='run only the WCAG 2.2 AA leg (scripts/test_app_a11y.py, lane L18)')
    ap.add_argument('--live-serve', action='store_true', help='run the http leg against the real loomward-serve binary')
    a = ap.parse_args()
    if a.live_serve:
        # The live legs serve app/dist, which is gitignored and goes stale (e.g. after a merge): rebuild it first.
        print('live serve: rebuilding app/dist before the live legs', flush=True)
        build_app(None, development=False)
    if not (DIST / 'index.html').exists():
        sys.exit('app/dist is missing: run `npm.cmd --prefix app run build` first')
    subprocess.run([sys.executable, str(ROOT / 'scripts/test_app_timing.py')], check=True)
    if a.live_serve:
        return run_live_serve(a.browser)

    DIST = ROOT / '.loomward/app-e2e'
    build_app(DIST, development=True)
    assert (DIST / 'index.html').is_file(), 'development fixtures were not built'
    test_app_views.DIST = DIST  # the views' fake engine serves this build, the one just made

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{server.server_port}'
    if a.a11y:
        with sync_playwright() as pw:
            browser = pw.chromium.launch(executable_path=a.browser, headless=True, args=['--no-sandbox'])
            run_a11y(browser, base, check)
            browser.close()
        server.shutdown()
        print('ALL PASS (a11y)')
        return
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

        # Duplicate evidence is synthetic and observed, never advice to delete.
        nav.get_by_role('link', name='Duplicates', exact=True).click()
        expect(page.get_by_role('heading', name='Duplicates', level=1)).to_be_visible()
        expect(page.get_by_text('Equal content is an observation, not a recommendation to delete.', exact=True)).to_be_visible()
        page.get_by_role('button', name='Start inspection').click()
        expect(page.get_by_role('button', name='Cancel inspection')).to_be_enabled()
        expect(page.get_by_role('rowheader', name='equal-a.txt equal-b.txt')).to_be_visible()
        expect(page.get_by_role('rowheader', name='linked.txt alias.txt')).to_be_visible()
        expect(page.get_by_text('One object · no space to reclaim', exact=True)).to_be_visible()
        expect(page.get_by_text('changed_during_read', exact=True)).to_be_visible()
        expect(page.get_by_role('row').filter(has=page.get_by_role('rowheader', name='equal-a.txt equal-b.txt'))).to_contain_text('16 B')
        expect(page.locator('.evidence')).to_contain_text('Bytes read 96 B')
        expect(page.locator('.evidence')).to_contain_text('Partial yes')
        expect(page.locator('.evidence')).to_contain_text('Stop none')
        expect(page.locator('#duplicate-results')).to_be_focused()
        shot('duplicates-desktop.png')
        page.set_viewport_size({'width': 375, 'height': 800})
        check(page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'duplicates reflows at 375 px')
        shot('duplicates-mobile.png')
        page.set_viewport_size({'width': 1360, 'height': 900})
        page.get_by_role('button', name='Start inspection').click()
        page.get_by_role('button', name='Cancel inspection').click()
        expect(page.get_by_role('status').filter(has_text='Inspection cancelled')).to_be_visible()
        print('PASS duplicates: verified sizes, link sets, skip reasons, evidence, focus and cancellation')
        nav.get_by_role('link', name='Explorer', exact=True).click()

        # search pages with a keyset cursor: 'delta' matches far more than one page
        page.get_by_placeholder('Search names').fill('delta')
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
        page.locator('main table tbody tr th button.crumb').first.click()
        expect(page.locator('main nav[aria-label="Location"] button')).to_have_count(2)
        page.locator('main nav[aria-label="Location"] button').first.click()
        expect(page.locator('main nav[aria-label="Location"] button')).to_have_count(1)
        print('PASS drill down and breadcrumb back')

        # --- routes ----------------------------------------------------------------------------
        page.get_by_role('navigation', name='Views').get_by_role('link', name='Grants & health').click()
        expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
        expect(page.get_by_text('Synthetic Alpha', exact=False).first).to_be_visible()
        check(page.get_by_text('off', exact=True).count() == 10, 'all 10 file/process effect capabilities show as off')
        expect(page.locator('.caps li').filter(has_text='Content read: duplicates only, on request')).to_contain_text('not available')
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

        meaning_weft(page, base)
        page.goto(f'{base}/?transport=mock#/explorer')

        # --- themes ----------------------------------------------------------------------------
        atlas = theme_snapshot(page)
        page.get_by_role('button', name='Observatory').click()
        expect(page.locator('html')).to_have_attribute('data-theme', 'observatory')
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
        expect(page.locator('html')).to_have_attribute('data-theme', 'woven-atlas')
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
        # Connected follows session.hello; wait for the stream too before deliberately dropping it.
        with page.expect_response(lambda r: r.url.endswith('/api/v3/events') and r.status == 200):
            page.reload()
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
        print('PASS a reload of the tab keeps the session without the fragment')

        engine.down = True
        expect(page.get_by_role('alert').filter(has_text='Unavailable')).to_contain_text('Unavailable', timeout=10000)
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Dataset unknown')
        check(page.locator('main h1', has_text='Explorer').count() == 0, 'the view is gone while the engine is down')
        engine.down = False
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected', timeout=20000)
        expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
        print('PASS drop to unavailable and recover')
        # the reconnect resumes with the full frame id `<epoch>.<seq>`; a bare seq makes the real service answer epoch_changed
        check_resume(page)

        # --- product views (lane L13): Explorer, Tiers, Companion, Grants & health ------------------
        run_collections(page, base)
        run_views(page, base, a.view_shots)
        run_inventory(page, base, check, shots=a.view_shots)
        run_copy(page, base, a.view_shots)
        run_ask(page, base)

        # --- hygiene -------------------------------------------------------------------------------
        check(hosts <= {'127.0.0.1'}, f'every request stayed on loopback: {sorted(hosts)}')
        check(not errors, f'no console errors or page errors: {errors}')
        check(not dialogs, 'no dialog ever opened')
        browser.close()
    server.shutdown()
    print('ALL PASS')


if __name__ == '__main__':
    main()
