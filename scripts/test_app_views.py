"""Browser checks for the product views of app/ (lane L13): Explorer, Tiers, Companion, Grants & health.

Called from scripts/test_app.py (`run_views`). Mock-transport scenarios run against the page that
test_app.py already serves; the Explorer's stale-response and invalidation scenarios need an engine
that can be slow or fail on demand, so this module starts its own tiny loopback fake engine.
Requires a built app/dist (`npm.cmd --prefix app run build`).
"""
from __future__ import annotations

import json
import queue
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import Page, expect

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / 'app' / 'dist'
TOKEN = '0f1e2d3c4b5a69788796a5b4c3d2e1f0'
EPOCH = 'e_fake'
MIME = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml'}
VIEWPORTS = {'1440': (1440, 900), '390': (390, 844)}

_HIDDEN = [(0x200B, 0x200F), (0x202A, 0x202E), (0x2060, 0x2069), (0xFEFF, 0xFEFF)]  # zero-width and bidi controls
CONTROL_CHARS = re.compile('[' + ''.join(f'{chr(a)}-{chr(b)}' for a, b in _HIDDEN) + ']')
FORBIDDEN_BUTTONS = re.compile(r'kill|suspend|trim|terminate|end task|priority|execute|apply|\bmove\b|\brun\b|delete', re.I)


def ok(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)
    print('PASS', msg)


def rows(page: Page):
    return page.locator('main table.tbl tbody tr')


# --- a fake engine that can be slow or fail on demand -------------------------------------------------

class FakeEngine:
    """Answers the few commands the Explorer needs. Flags are flipped by the test."""

    def __init__(self) -> None:
        self.fail_children = False
        self.stale_once = False
        self.plan_mode = 'executable'  # 'executable' or 'malformed': how placement.simulate misbehaves
        self.slice_queue: list[tuple[float, str]] = []  # (delay, root name) per tree.slice call, then the default
        self.events: queue.Queue[dict] = queue.Queue()
        self.seq = 0
        self.server: ThreadingHTTPServer | None = None

    @property
    def base(self) -> str:
        assert self.server
        return f'http://127.0.0.1:{self.server.server_port}'

    def invalidate(self) -> None:
        self.seq += 1
        self.events.put({'protocol': 'loomward/3', 'epoch': EPOCH, 'seq': self.seq, 'event': 'tree.invalidated', 'at': '2026-10-01T00:00:00Z', 'catalog_rev': str(self.seq + 1), 'state_rev': None, 'data': {'root_id': None, 'generation': None, 'scope': 'all', 'node_ids': []}})

    def result(self, req: dict):
        cmd, p = req['command'], req.get('payload') or {}
        row = lambda name, i: {'node_id': f'nd_{name}', 'kind': 'file', 'name': name, 'extension': 'txt', 'ext_family': 'document', 'logical_bytes': str(1000 + i), 'allocated_bytes': '4096', 'files': None, 'dirs': None, 'modified_at': '2026-01-01T00:00:00Z', 'attributes': [], 'flags': [], 'coverage': 'complete', 'location_hint': None}
        if cmd == 'session.hello':
            effects = {k: False for k in 'file_move file_delete file_rename file_write content_read process_kill process_suspend process_priority memory_trim uninstall elevation'.split()}
            return {
                'protocol': 'loomward/3', 'engine_version': 'view-fake', 'adapter': 'http', 'dataset_class': 'synthetic', 'session_started_at': '2026-10-01T00:00:00Z',
                'enumeration_strategy': 'std_read_dir', 'capabilities': {'observation': {}, 'effects': effects},
                'features': {k: False for k in ('grant_picker', 'disclosure_dialog', 'teacher_available', 'telemetry_available', 'gpu_available')},
                'limits': {'max_request_bytes': 65536, 'max_slice_nodes': 6000, 'max_page_items': 200},
            }
        if cmd == 'tree.slice':
            delay, root = self.slice_queue.pop(0) if self.slice_queue else (0.0, 'Fake Root')
            time.sleep(delay)
            return {'anchor_node_id': 'nd_atlas', 'basis': 'logical', 'root_generations': [], 'complete': True, 'live': False, 'ordering': 'exact', 'truncated': False,
                    'nodes': [{'node_id': 'nd_atlas', 'parent': None, 'name': 'Atlas'}, {'node_id': 'nd_' + root.replace(' ', '_'), 'parent': 0, 'name': root}]}
        if cmd == 'tree.children':
            if self.fail_children:
                return ('internal_error', 'fake engine: refresh failed')
            start = int(p['cursor'][2:]) if p.get('cursor') else 0
            if p.get('cursor') and self.stale_once:
                self.stale_once = False
                return ('stale_generation', 'fake engine: the folder changed under the cursor')
            end = min(start + p['limit'], 120)
            return {'anchor': p['node_id'], 'generation': '7', 'items': [row(f'item-{i:03d}.txt', i) for i in range(start, end)], 'next_cursor': f'c_{end}' if end < 120 else None, 'total': 120, 'budget_hit': False}
        if cmd == 'tiers.model':
            tier = {'tier': 1, 'basis': 'device_hint', 'declared_tier': None, 'hint_tier': 1, 'note': 'hint'}
            return {'volumes': [{'volume_id': 'vo_x', 'display_name': 'X:', 'tier': tier, 'online': True, 'writable': True, 'capacity_bytes': '1000000000000', 'free_bytes': '90000000000', 'reserve_bytes': '1', 'free_fraction': 0.09, 'pressure': 'pressure'}],
                    'policy': {'watch_free_fraction': 0.2, 'pressure_free_fraction': 0.1, 'reserve_note': 'r', 'note': 'n'}}
        if cmd == 'placement.candidates':
            g = {'group_id': 'cg_a', 'node_id': 'nd_a', 'root_id': 'rt_a', 'name': 'group-a', 'source_bytes': '5000', 'destination_bytes': '5000', 'transfer_bytes': '5000', 'estimate_basis': 'allocated_entries',
                 'estimated_relief_bytes': '5000', 'relief_basis': 'verified_unique_allocation', 'heat': None, 'heat_basis': 'unknown', 'newest_modified_at': None, 'pinned': False, 'active': False, 'protected': False, 'days_since_move': 30, 'coverage': 'complete'}
            return {'source_volume_id': 'vo_x', 'root_generations': [], 'groups': [g], 'note': 'n'}
        if cmd == 'placement.simulate':
            plan = {'mode': 'simulation', 'algorithm': 'bounded_portfolio_search_v2', 'optimality_claim': False, 'optimality_scope': 's', 'shortfall_optimal': True,
                    'search': {'complete': True, 'nodes_visited': 1, 'node_budget': 10, 'reason': 'exhausted', 'eligible_groups': 1, 'eligible_targets': 1, 'lower_bound_shortfall_bytes': '0'},
                    'proposals': [{'group_id': 'cg_a', 'source_id': 'vo_x', 'target_id': 'vo_y', 'source_bytes_relieved': '5000', 'destination_bytes_required': '5000', 'transfer_bytes': '5000', 'reason': 'bounded_capacity_budget_allocation', 'requires_consent': True, 'executable': True}],
                    'rejected': [], 'pre_rejected': [], 'excluded_volumes': [], 'relief_policy': 'verified_only', 'projected_free_bytes': {}, 'target_free_bytes': '1', 'shortfall_bytes': '0', 'satisfied': True,
                    'transfer_bytes': '5000', 'filesystem_changed': False, 'baseline_shortfall_bytes': '777', 'shortfall_improvement_bytes': '777', 'assumption': 'a', 'heat_policy': p['heat_policy'],
                    'assumptions': [], 'root_generations': [], 'proposal_id': None}
            if self.plan_mode == 'malformed':
                plan['proposals'] = 'none'
            return plan
        if cmd == 'search.query':
            text = p.get('text', '')
            if text == 'slow':
                time.sleep(1.5)
            name = 'SLOW-RESULT.txt' if text == 'slow' else 'FAST-RESULT.txt'
            return {'anchor': None, 'generation': '7', 'items': [{**row(name, 1), 'location_hint': {'text': 'Fake Root', 'truncated': False}}], 'next_cursor': None, 'total': 1, 'budget_hit': False}
        return ('capability_unavailable', f'fake engine: {cmd} not implemented')

    def start(self) -> None:
        engine = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.0'

            def log_message(self, *a):
                pass

            def _send(self, status: int, body: bytes, ctype: str) -> None:
                try:
                    self.send_response(status)
                    self.send_header('Content-Type', ctype)
                    self.send_header('Content-Length', str(len(body)))
                    self.send_header('Cache-Control', 'no-store')
                    self.end_headers()
                    self.wfile.write(body)
                except OSError:
                    pass  # the page aborted this request (a superseded search): nobody is listening

            def do_GET(self) -> None:
                path = urlsplit(self.path).path
                if path == '/api/v3/events':
                    if self.headers.get('X-Loomward-Token') != TOKEN:
                        return self._send(403, b'{}', 'application/json')
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Cache-Control', 'no-store')
                    self.end_headers()
                    try:
                        hello = {'protocol': 'loomward/3', 'epoch': EPOCH, 'seq': engine.seq, 'event': 'stream.hello', 'at': '2026-10-01T00:00:00Z', 'catalog_rev': None, 'state_rev': None,
                                 'data': {'session_started_at': '2026-10-01T00:00:00Z', 'epoch': EPOCH, 'last_seq': engine.seq, 'oldest_replayable_seq': 1, 'dataset_class': 'synthetic'}}
                        self.wfile.write(f'id: {EPOCH}.{engine.seq}\nevent: stream.hello\ndata: '.encode() + json.dumps(hello).encode() + b'\n\n')
                        self.wfile.flush()
                        while True:
                            try:
                                ev = engine.events.get(timeout=0.1)
                                self.wfile.write(f"id: {ev['epoch']}.{ev['seq']}\nevent: {ev['event']}\ndata: {json.dumps(ev)}\n\n".encode())
                            except queue.Empty:
                                self.wfile.write(b': heartbeat\n\n')
                            self.wfile.flush()
                    except OSError:
                        pass
                    return
                file = (DIST / ('index.html' if path in ('/', '') else path.lstrip('/'))).resolve()
                try:
                    if DIST.resolve() not in file.parents and file != DIST.resolve() / 'index.html':
                        raise FileNotFoundError
                    self._send(200, file.read_bytes(), MIME.get(file.suffix, 'application/octet-stream'))
                except OSError:
                    self._send(404, b'not found', 'text/plain')

            def do_POST(self) -> None:
                if urlsplit(self.path).path != '/api/v3/call' or self.headers.get('X-Loomward-Token') != TOKEN:
                    return self._send(403, b'{}', 'application/json')
                req = json.loads(self.rfile.read(int(self.headers.get('Content-Length', '0'))))
                res = engine.result(req)
                base = {'protocol': 'loomward/3', 'request_id': req['request_id']}
                if isinstance(res, tuple):
                    out = {**base, 'ok': False, 'error': {'code': res[0], 'message': res[1], 'retryable': res[0] == 'stale_generation', 'detail': None}}
                else:
                    out = {**base, 'ok': True, 'result': res, 'meta': {'served_at': '2026-10-01T00:00:00Z', 'elapsed_ms': 0, 'dataset_class': 'synthetic', 'budget_hit': False, 'catalog_rev': '1', 'state_rev': None}}
                self._send(200, json.dumps(out).encode(), 'application/json')

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def stop(self) -> None:
        if self.server:
            self.server.shutdown()


# --- Explorer ---------------------------------------------------------------------------------------

def check_explorer_mock(page: Page, base: str, shoot) -> None:
    page.goto(f'{base}/?transport=mock#/explorer')
    expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
    expect(rows(page).first).to_be_visible()
    main = page.locator('main')

    # hidden and bidi characters are shown as badges and never reach the DOM as themselves
    expect(main.get_by_text('U+202E RLO', exact=True).first).to_be_visible()  # the corpus also has an RLO folder (L10)
    expect(main.get_by_text('U+200B ZWSP', exact=True)).to_be_visible()
    expect(main.get_by_text('U+200D ZWJ', exact=True)).to_be_visible()
    ok(CONTROL_CHARS.search(main.inner_text()) is None, 'no bidi or zero-width character is left in the rendered text, only badges')
    ok(page.get_by_text('hidden characters', exact=True).count() >= 2, 'rows with hidden characters are tagged')
    shoot('explorer')

    # sort
    first = lambda: rows(page).first.locator('td').first.inner_text()
    by_size = first()
    page.get_by_label('Sort').select_option('name_asc')
    expect(rows(page).first.locator('td').first).not_to_have_text(by_size)
    ok(first().startswith('"><svg/onload=alert(1)>'), 'name sort puts the quote-leading name first')
    page.get_by_label('Sort').select_option('size_desc')
    expect(rows(page).first.locator('td').first).to_have_text(by_size)
    print('PASS sort by name and back to size')

    # basis
    page.get_by_role('button', name='Allocated', exact=True).click()
    expect(page.get_by_role('button', name='Allocated', exact=True)).to_have_attribute('aria-pressed', 'true')
    expect(page.locator('th.basis')).to_have_text('Allocated')
    page.get_by_role('button', name='Logical', exact=True).click()
    expect(page.locator('th.basis')).to_have_text('Logical')
    print('PASS size basis toggles and marks its column')

    # search filters
    page.get_by_placeholder('Search names').fill('a')
    page.get_by_label('Kind').select_option('dir')
    page.get_by_role('button', name='Search', exact=True).click()
    expect(page.get_by_text('Search results for')).to_be_visible()
    kinds = rows(page).locator('td:nth-child(2)').all_inner_texts()
    ok(len(kinds) > 0 and set(kinds) == {'dir'}, 'kind filter keeps folders only')
    page.get_by_label('Kind').select_option('any')
    page.get_by_placeholder('Search names').fill('')
    page.get_by_label('Extension').fill('pdf')
    page.get_by_role('button', name='Search', exact=True).click()
    names = rows(page).locator('td:first-child').all_inner_texts()
    ok(len(names) > 0 and all(n.split('\n')[0].endswith('.pdf') for n in names), 'extension filter alone searches and matches only that extension')
    page.get_by_label('Extension').fill('')
    page.get_by_role('button', name='Clear search').click()
    expect(page.locator('main nav[aria-label="Location"]')).to_be_visible()


def check_explorer_engine(page: Page, engine: FakeEngine) -> None:
    page.goto(f'{engine.base}/#token={TOKEN}')
    page.evaluate("location.hash = '#/explorer'")
    expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
    expect(rows(page)).to_have_count(50)

    # a slow answer for an old search never overwrites a newer one
    box = page.get_by_placeholder('Search names')
    box.fill('slow')
    page.get_by_role('button', name='Search', exact=True).click()
    box.fill('fast')
    page.get_by_role('button', name='Search', exact=True).click()
    expect(page.get_by_text('FAST-RESULT.txt')).to_be_visible()
    page.wait_for_timeout(2000)
    ok(page.get_by_text('SLOW-RESULT.txt').count() == 0 and page.get_by_text('FAST-RESULT.txt').count() == 1, 'a slow reply for an older search is dropped')
    page.get_by_role('button', name='Clear search').click()
    expect(rows(page)).to_have_count(50)

    # a cursor that went stale restarts the listing from the top, with a notice
    engine.stale_once = True
    page.get_by_role('button', name='Load more').click()
    expect(page.get_by_text('starts again from the top')).to_be_visible()
    expect(rows(page)).to_have_count(50)
    page.get_by_role('button', name='Load more').click()
    expect(rows(page)).to_have_count(100)
    print('PASS a stale cursor restarts from the first page, then paging continues')

    # tree.invalidated followed by a failed refresh: the error shows and no stale row stays
    engine.fail_children = True
    engine.invalidate()
    expect(page.get_by_role('alert').filter(has_text='refresh failed')).to_be_visible()
    ok(page.get_by_text('item-000.txt').count() == 0, 'after tree.invalidated and a failed refresh no stale row remains')
    ok('Showing 0 of unknown' in page.locator('main').inner_text(), 'the count falls back to 0 of unknown, not the old total')
    engine.fail_children = False
    engine.invalidate()
    expect(page.get_by_text('item-000.txt')).to_be_visible()
    expect(page.get_by_role('alert').filter(has_text='refresh failed')).to_have_count(0)
    print('PASS the next successful refresh brings the rows back and clears the error')


def check_explorer_starts(page: Page, engine: FakeEngine) -> None:
    # the first tree.slice is slow and a tree.invalidated starts a second, fast one: the old answer must not win
    engine.slice_queue = [(1.5, 'Old Root'), (0.0, 'Fake Root')]
    page.goto(f'{engine.base}/#token={TOKEN}')
    page.evaluate("location.hash = '#/explorer'")
    expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
    page.wait_for_timeout(300)
    engine.invalidate()
    expect(page.locator('main nav[aria-label="Location"]')).to_contain_text('Fake Root')
    page.wait_for_timeout(2000)
    ok(page.locator('main nav[aria-label="Location"]').inner_text().strip() == 'Fake Root' and page.get_by_text('Old Root').count() == 0, 'a slow starting-point reply for an older session state is dropped')


def check_tiers_refused(page: Page, engine: FakeEngine) -> None:
    # every reply is unsound: the page must refuse them whole, show no figure from them, and not crash
    for mode, note in (('executable', 'an executable proposal'), ('malformed', 'a malformed plan')):
        engine.plan_mode = mode
        page.goto(f'{engine.base}/#token={TOKEN}')
        page.evaluate("location.hash = '#/tiers'")
        expect(page.get_by_role('heading', name='Tiers', level=1)).to_be_visible()
        expect(rows(page).first).to_be_visible()
        page.get_by_role('button', name='Simulate', exact=True).click()
        expect(page.get_by_role('heading', name='Alternatives')).to_be_visible()
        expect(page.get_by_text('No trustworthy plan: nothing to compare').first).to_be_visible()
        text = page.locator('main').inner_text()
        ok('777' not in text and 'Already met' not in text and 'Not met' not in text, f'{note} is refused: no baseline or result figure is shown from it')
        page.get_by_role('row', name=re.compile('^Do nothing')).get_by_role('button', name='Show').click()
        expect(page.get_by_role('heading', name='Do nothing', level=2)).to_be_visible()
        expect(page.get_by_text('No trustworthy plan: nothing to compare').first).to_be_visible()
        ok('short of the target' not in page.locator('main').inner_text(), f'{note}: the do-nothing section makes no claim about the target')


# --- Tiers ------------------------------------------------------------------------------------------

def check_tiers(page: Page, base: str, shoot) -> None:
    page.goto(f'{base}/?transport=mock#/tiers')
    expect(page.get_by_role('heading', name='Tiers', level=1)).to_be_visible()
    for v in ('C:', 'G:', 'E:', 'F:'):
        expect(page.get_by_role('heading', name=v, level=3)).to_be_visible()
    main = page.locator('main')
    expect(main.get_by_text('Tier 1 · hinted')).to_be_visible()
    expect(main.get_by_text('Tier 2 · declared')).to_be_visible()
    expect(main.get_by_text('Tier unknown').first).to_be_visible()
    expect(main.get_by_text('Under pressure')).to_be_visible()
    expect(page.get_by_role('button', name='Source volume')).to_have_count(1)
    ok(page.get_by_role('button', name='Use as source').and_(page.locator('[disabled]')).count() == 1, 'the unknown-tier volume cannot be chosen as a source')
    expect(rows(page).first).to_be_visible()
    ok(page.get_by_text('synthetic-model-store').count() >= 1, 'candidate groups are listed for the source volume')
    ok(page.get_by_text('unknown', exact=True).count() >= 3, 'unknown relief and heat stay unknown')
    shoot('tiers')

    page.get_by_role('button', name='Simulate', exact=True).click()
    expect(page.get_by_text('Simulation · executable: false · filesystem_changed: false')).to_be_visible()
    expect(page.get_by_role('heading', name='Alternatives')).to_be_visible()
    expect(page.get_by_role('rowheader', name=re.compile('^Do nothing'))).to_be_visible()
    expect(page.get_by_text('No group qualified under this policy')).to_be_visible()
    expect(page.get_by_text('Heat is unknown').first).to_be_visible()
    expect(page.get_by_text('Shares file objects with another group')).to_be_visible()
    expect(page.get_by_text('Tier unknown').last).to_be_visible()  # F: left out of the plan
    ok(main.get_by_role('button', name=FORBIDDEN_BUTTONS).count() == 0, 'no button anywhere on Tiers can move, apply or run anything')

    # an owner what-if produces a proposal that is consent-gated and not executable
    model_row = rows(page).filter(has_text='synthetic-model-store').first
    model_row.get_by_role('combobox').select_option('0.1')
    model_row.get_by_role('checkbox').check()
    page.get_by_role('button', name='Simulate', exact=True).click()
    expect(page.get_by_text('Target met in the simulation')).to_be_visible()
    expect(main.get_by_text('consent required').first).to_be_visible()
    expect(main.get_by_text('not executable').first).to_be_visible()
    expect(main.get_by_text('assumed 0.1').first).to_be_visible()
    shoot('tiers-simulation')

    # do nothing is a first-class alternative
    page.get_by_role('row', name=re.compile('^Do nothing')).get_by_role('button', name='Show').click()
    expect(page.get_by_role('heading', name='Do nothing', level=2)).to_be_visible()
    print('PASS Tiers: simulation label, alternatives with do nothing, what-if assumptions, no effect controls')


# --- Companion --------------------------------------------------------------------------------------

def check_companion(page: Page, base: str, shoot) -> None:
    page.goto(f'{base}/?transport=mock#/companion')
    expect(page.get_by_role('heading', name='Companion', level=1)).to_be_visible()
    main = page.locator('main')
    expect(main.get_by_text('Observation only')).to_be_visible()
    expect(rows(page).first).to_be_visible()
    ok(main.get_by_role('button', name=FORBIDDEN_BUTTONS).count() == 0, 'no control to end, pause, re-prioritise or trim anything')

    plist = lambda: page.locator('table.procs tbody tr')
    expect(plist()).to_have_count(20)
    expect(main.get_by_text(re.compile(r'Showing 20 of \d+ observed'))).to_be_visible()
    ok(plist().first.inner_text().startswith('vm-host.exe'), 'processes are sorted by private commit, largest first')

    page.get_by_label('Show').select_option('100')
    expect(plist().nth(30)).to_be_visible()
    denied = plist().filter(has_text='protected-service.exe')
    expect(denied).to_have_count(1)
    expect(denied).to_contain_text('access denied')
    expect(denied.locator('.unknown').first).to_be_visible()
    ok(denied.get_by_text('0 B').count() == 0, 'an unreadable process shows unknown, never zero')
    expect(plist().filter(has_text='svchost').get_by_text('U+200B ZWSP')).to_be_visible()
    expect(plist().filter(has_text='Loomward').first).to_be_visible()

    # memory ledger: the three splits the engine does not report are unknown; commit is separate
    ledger = main.locator('table.ledger')
    for cat in ('Modified', 'Standby', 'Free'):
        expect(ledger.get_by_role('row', name=re.compile(f'^{cat}')).get_by_text('unknown').first).to_be_visible()
    expect(ledger.get_by_role('row', name=re.compile('^In use'))).to_contain_text('derived')
    expect(main.get_by_role('heading', name=re.compile('^Commit charge'))).to_be_visible()

    # explanations are rule-based text with no action
    plist().filter(has_text='vm-host.exe').first.get_by_role('button').click()
    region = page.get_by_role('region', name='Explanation')
    expect(region).to_contain_text('Loomward offers no action')
    expect(region.get_by_role('button')).to_have_count(0)

    # own budgets are shown and read-only
    expect(main.get_by_role('heading', name='Worker budgets')).to_be_visible()
    expect(main.get_by_role('row', name=re.compile('^scan enumerate'))).to_be_visible()
    ok(page.locator('section[aria-labelledby="h-own"] input, section[aria-labelledby="h-own"] button').count() == 0, 'Loomward budgets have no controls')

    # the view keeps sampling
    seq = lambda: int(re.search(r'Sample ([\d,]+) at', main.inner_text()).group(1).replace(',', ''))
    before = seq()
    page.wait_for_function('(b) => { const m = /Sample ([\\d,]+) at/.exec(document.querySelector("main").innerText); return m && Number(m[1].replace(/,/g, "")) > b; }', arg=before, timeout=9000)
    print('PASS Companion: bounded list with coverage, unknowns with reasons, split ledger, explanation, read-only budgets, live sampling')
    shoot('companion')


# --- Grants & health --------------------------------------------------------------------------------

def check_health(page: Page, base: str, shoot) -> None:
    page.goto(f'{base}/?transport=mock#/health')
    expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
    main = page.locator('main')
    expect(main.get_by_role('heading', name="Loomward's own health")).to_be_visible()
    expect(main.get_by_text('Private commit')).to_be_visible()
    expect(main.get_by_text('Writer queue')).to_be_visible()
    ok(main.get_by_text('off', exact=True).count() == 11, 'the capability matrix shows all 11 effects off')
    ok(main.get_by_text('ON', exact=True).count() == 0, 'no effect is on')
    expect(main.get_by_text('not available').first).to_be_visible()
    shoot('health')

    # revoke a root: asked, confirmed, shown; files untouched
    roots_table = main.locator('section[aria-labelledby="h-roots"] table')
    first_root = roots_table.locator('tbody tr').first
    first_root.get_by_role('button', name='Revoke…').click()
    dialog = page.get_by_role('group', name=re.compile('Revoke root access'))
    expect(dialog).to_be_focused()
    expect(dialog).to_contain_text('The files on disk are never touched')
    dialog.get_by_role('button', name='Keep it').click()
    expect(dialog).to_have_count(0)
    first_root.get_by_role('button', name='Revoke…').click()
    dialog.get_by_label(re.compile('Also delete')).check()
    dialog.get_by_role('button', name='Revoke', exact=True).click()
    expect(page.get_by_role('status').filter(has_text='Revoked')).to_contain_text('catalogue rows for it were deleted')
    expect(first_root).to_contain_text('revoked')
    expect(first_root.get_by_role('button', name='Revoke…')).to_be_disabled()

    # revoke the teacher disclosure grant
    grants = main.locator('section[aria-labelledby="h-grants"]')
    grants.locator('li', has_text='Teacher disclosure').get_by_role('button', name='Revoke…').click()
    page.get_by_role('group', name=re.compile('Revoke grant')).get_by_role('button', name='Revoke', exact=True).click()
    expect(grants.locator('li', has_text='Teacher disclosure')).to_contain_text('revoked')
    expect(grants.locator('li', has_text='Teacher disclosure').get_by_role('button')).to_have_count(0)
    print('PASS Grants & health: revoke root and grant with confirmation, state reflected, all effects off')


def check_narrow(page: Page, base: str) -> None:
    page.set_viewport_size({'width': 390, 'height': 844})
    for view in ('explorer', 'tiers', 'companion', 'health'):
        page.goto(f'{base}/?transport=mock#/{view}')
        expect(page.locator('main h1')).to_be_visible()
        page.wait_for_timeout(400)
        ok(page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), f'no horizontal page scroll at 390 px on {view}')
    page.set_viewport_size({'width': 1360, 'height': 900})


WIDE_FONTS = {'Courier New': '"Courier New", monospace', 'Verdana': 'Verdana, sans-serif'}
FONT_CSS = ':root, :root[data-theme] { --font-ui: %(f)s !important; --font-display: %(f)s !important; --font-mono: %(f)s !important; } * { font-family: %(f)s !important; }'


def check_narrow_wide_fonts(page: Page, base: str) -> None:
    """Hosted CI has none of the Windows fonts, and users have other fonts and text scaling: wide fallback
    faces must never push a view past the viewport. Fonts are forced with a constructed stylesheet (the
    page's CSP forbids inline <style>)."""
    for width in (390, 320):
        page.set_viewport_size({'width': width, 'height': 844})
        for label, family in WIDE_FONTS.items():
            page.goto(f'{base}/?transport=mock#/explorer')
            page.reload()  # a same-URL goto is only a hash change: reload for a fresh mock (earlier checks revoked roots)
            expect(page.locator('main h1')).to_be_visible()
            page.evaluate('(css) => { const s = new CSSStyleSheet(); s.replaceSync(css); document.adoptedStyleSheets = [s]; }', FONT_CSS % {'f': family})
            for view in ('explorer', 'tiers', 'companion', 'health'):
                page.evaluate(f"location.hash = '#/{view}'")
                expect(page.locator('main h1')).to_be_visible()
                page.wait_for_timeout(500)
                if view == 'tiers':
                    page.get_by_role('button', name='Simulate', exact=True).click()
                    expect(page.get_by_role('heading', name='Alternatives')).to_be_visible()
                if view == 'companion':
                    page.locator('table.procs tbody tr').first.get_by_role('button').click()
                    expect(page.get_by_role('region', name='Explanation')).to_be_visible()
                if view == 'health':
                    page.get_by_role('button', name='Revoke…').first.click()
                    expect(page.get_by_role('group', name=re.compile('Revoke root access'))).to_be_visible()
                page.wait_for_timeout(150)
                over = page.evaluate('document.documentElement.scrollWidth - window.innerWidth')
                ok(over <= 0, f'no horizontal page scroll at {width} px in {label} on {view} (over by {over})')
    page.set_viewport_size({'width': 1360, 'height': 900})


def run_views(page: Page, base: str, shots: Path | None) -> None:
    """Runs every L13 scenario. `shots` is the evidence directory, or None to skip screenshots."""
    if shots:
        shots.mkdir(parents=True, exist_ok=True)

    def shoot(name: str) -> None:
        if not shots:
            return
        for tag, (w, h) in VIEWPORTS.items():
            page.set_viewport_size({'width': w, 'height': h})
            page.wait_for_timeout(350)
            page.screenshot(path=str(shots / f'{name}-{tag}.png'), full_page=True)
        page.set_viewport_size({'width': 1360, 'height': 900})

    page.set_viewport_size({'width': 1360, 'height': 900})
    check_explorer_mock(page, base, shoot)
    engine = FakeEngine()
    engine.start()
    try:
        check_explorer_engine(page, engine)
        check_explorer_starts(page, engine)
        check_tiers_refused(page, engine)
    finally:
        engine.stop()
    check_tiers(page, base, shoot)
    check_companion(page, base, shoot)
    check_health(page, base, shoot)
    check_narrow(page, base)
    check_narrow_wide_fonts(page, base)
