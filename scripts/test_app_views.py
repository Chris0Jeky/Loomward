"""Browser checks for the product views of app/ (lane L13): Explorer, Tiers, Companion, Grants & health, Backups.

Called from scripts/test_app.py (`run_views`). Mock-transport scenarios run against the page that
test_app.py already serves; the Explorer's stale-response and invalidation scenarios need an engine
that can be slow or fail on demand, so this module starts its own tiny loopback fake engine.
Serves the development build that test_app.py makes in .loomward/app-e2e, never app/dist (which can be stale).
"""
from __future__ import annotations

import json
import queue
import re
import threading
import time
from collections.abc import Callable
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import Page, expect

ROOT = Path(__file__).resolve().parents[1]
# The same directory test_app.py builds (.loomward/app-e2e). test_app.py also assigns this before running any leg,
# so the fake engine's page is the build under test, not a stale app/dist.
DIST = ROOT / '.loomward' / 'app-e2e'
TOKEN = '0f1e2d3c4b5a69788796a5b4c3d2e1f0'
EPOCH = 'e_fake_epoch'
MIME = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml'}
VIEWPORTS = {'1440': (1440, 900), '390': (390, 844)}

_HIDDEN = [(0x200B, 0x200F), (0x202A, 0x202E), (0x2060, 0x2069), (0xFEFF, 0xFEFF)]  # zero-width and bidi controls
CONTROL_CHARS = re.compile('[' + ''.join(f'{chr(a)}-{chr(b)}' for a, b in _HIDDEN) + ']')
FORBIDDEN_BUTTONS = re.compile(r'kill|suspend|trim|terminate|end task|priority|execute|apply|\bmove\b|\brun\b|delete', re.I)



def wait_until(page, expression: str | Callable[[], bool], arg=None, timeout_ms: int = 9000) -> None:
    # page.wait_for_function polls through page-side eval, which the app's CSP (no
    # 'unsafe-eval') blocks whenever the predicate is not already true; page.evaluate is not.
    deadline = time.monotonic() + timeout_ms / 1000
    while not (expression() if callable(expression) else page.evaluate(expression, arg)):
        if time.monotonic() > deadline:
            raise AssertionError(f'timed out waiting for: {expression}')
        # Python predicates still need to dispatch intercepted browser requests.
        page.wait_for_timeout(50)

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
        self.nested = False  # tree.children lists one sub-folder (nd_dir_N+1, down to nd_dir_3) beside a file
        self.node_prefix = ''  # a rebuild reissues IDs without changing folder names
        self.nested_depth = 3
        self.children_gate: tuple[str, threading.Event, threading.Event] | None = None
        self.gone: set[str] = set()  # node ids the engine no longer knows: tree.children answers not_found
        self.broken: dict[str, str] = {}  # node id -> error code tree.children answers instead of rows (e.g. an ancestor that fails)
        self.fail_proposals = False
        self.saved_scenarios = False
        self.served: dict[str, int] = {}  # command -> times answered, so a test can wait for a reload to be served
        self.candidate_gate = None  # one (arrived, release[, source_volume_id]) gate; a named source gates only its own request
        self.tier_volumes: list[dict] | None = None
        self.candidate_sources: list[str] = []
        self.placement_groups: dict[str, list[dict]] = {}
        self.saved_source = 'vo_x'
        self.simulate_overrides: list = []
        self.plan_mode = 'executable'  # 'executable', 'malformed', or a 'safe' nonexecutable reply
        self.slice_queue: list[tuple[float, str]] = []  # (delay, root name) per tree.slice call, then the default
        self.events: queue.Queue[dict] = queue.Queue()
        self.seq = 0
        self.server: ThreadingHTTPServer | None = None
        self.process_error: str | None = None
        self.telemetry_calls: list[dict] = []
        self.explain_partial_once = False
        self.explain_calls: list[str] = []
        self.process_observed_at: str | None = None
        self.backups: list[dict] | None = None  # set: hello says a personal desktop session, and backups.* answer
        self.backup_answers: list[dict] = []  # backups.import outcomes, in order
        self.backup_calls: list[dict] = []
        self.grant_picker = False  # session.hello features.grant_picker
        self.diagnostics_answers: list[dict] | None = None  # set: hello offers diagnostics_save, diagnostics.save pops these
        self.diagnostics_calls: list[dict] = []
        self.roots: list[dict] = []  # roots.list
        self.grant_mode = 'granted'  # roots.request_grant: granted, cancelled, refused, busy, deadline (the root lands before the reply) or deadline_late (after it)
        self.grant_delay = 0.0
        self.grant_calls: list[dict] = []
        self.trend_mode: str | None = None

    @property
    def base(self) -> str:
        assert self.server
        return f'http://127.0.0.1:{self.server.server_port}'

    def invalidate(self) -> None:
        self.seq += 1
        self.events.put({'protocol': 'loomward/3', 'epoch': EPOCH, 'seq': self.seq, 'event': 'tree.invalidated', 'at': '2026-10-01T00:00:00Z', 'catalog_rev': str(self.seq + 1), 'state_rev': None, 'data': {'root_id': None, 'generation': None, 'scope': 'all', 'node_ids': []}})

    def fake_root(self, name: str) -> dict:
        return {'root_id': f'rt_picked_{len(self.roots) + 1}', 'display_path': {'text': name, 'truncated': False}, 'origin': 'owner_granted', 'dataset_class': 'personal', 'volume_id': None,
                'granted_at': '2026-10-01T00:00:00Z', 'granted_via': 'desktop_picker', 'grant_state': 'active',
                'scan': {'state': 'never_scanned', 'last_job_id': None, 'generation': None, 'finished_at': None, 'coverage': 'unscanned'}, 'totals': None}

    def land(self, name: str) -> None:
        self.roots.append(self.fake_root(name))
        self.seq += 1
        self.events.put({'protocol': 'loomward/3', 'epoch': EPOCH, 'seq': self.seq, 'event': 'roots.changed', 'at': '2026-10-01T00:00:00Z', 'catalog_rev': str(self.seq + 1), 'state_rev': None, 'data': {'roots': self.roots}})

    def result(self, req: dict):
        cmd, p = req['command'], req.get('payload') or {}
        self.served[cmd] = self.served.get(cmd, 0) + 1
        row = lambda name, i: {'node_id': f'nd_{name}', 'kind': 'file', 'name': name, 'extension': 'txt', 'ext_family': 'document', 'logical_bytes': str(1000 + i), 'allocated_bytes': '4096', 'files': None, 'dirs': None, 'modified_at': '2026-01-01T00:00:00Z', 'attributes': [], 'flags': [], 'coverage': 'complete', 'location_hint': None}
        if cmd == 'session.hello':
            effects = {k: False for k in 'file_move file_delete file_rename file_write content_read process_kill process_suspend process_priority memory_trim uninstall elevation'.split()}
            desktop = self.backups is not None
            return {
                'protocol': 'loomward/3', 'engine_version': 'view-fake', 'adapter': 'tauri' if desktop else 'http',
                'dataset_class': 'personal' if desktop else 'synthetic', 'session_started_at': '2026-10-01T00:00:00Z',
                'enumeration_strategy': 'std_read_dir', 'capabilities': {'observation': {'duplicate_content_read': True, 'process_observation': self.process_error is not None}, 'effects': effects},
                'features': {**{k: k == 'grant_picker' and self.grant_picker for k in ('grant_picker', 'disclosure_dialog', 'teacher_available', 'telemetry_available', 'gpu_available')},
                             'diagnostics_save': self.diagnostics_answers is not None},
                'limits': {'max_request_bytes': 65536, 'max_slice_nodes': 6000, 'max_page_items': 200},
            }
        if self.backups is not None:
            if cmd == 'backups.list':
                return {'evidence': self.backups}
            if cmd == 'volumes.list':
                return {'volumes': []}
            if cmd == 'backups.import':
                self.backup_calls.append(req)
                answer = self.backup_answers.pop(0)
                if answer['outcome'] == 'imported':
                    self.backups.append(answer['evidence'])
                return answer
        if cmd == 'diagnostics.preview':
            return json.loads((ROOT / 'contracts/v3/examples/commands/diagnostics.preview.result.json').read_text(encoding='utf-8'))
        if cmd == 'diagnostics.save' and self.diagnostics_answers is not None:
            self.diagnostics_calls.append(req)
            answer = self.diagnostics_answers.pop(0)
            if isinstance(answer, dict) and answer.get('delay'):
                time.sleep(answer.pop('delay'))
            return answer
        if cmd == 'roots.list':
            return {'roots': self.roots}
        if cmd == 'grants.list':
            return {'grants': []}
        if cmd == 'jobs.list':
            return {'jobs': []}
        if cmd == 'health.get':
            return json.loads((ROOT / 'contracts/v3/examples/commands/health.get.result.json').read_text())
        if cmd == 'roots.request_grant':
            self.grant_calls.append(req)
            time.sleep(self.grant_delay)
            if self.grant_mode == 'cancelled':
                return {'outcome': 'cancelled_by_user', 'root': None, 'refusal': None}
            if self.grant_mode == 'refused':
                return {'outcome': 'refused', 'root': None, 'refusal': 'identity_unavailable'}
            if self.grant_mode == 'busy':
                return ('busy', 'a native dialog is already open')
            if self.grant_mode == 'deadline':
                self.land('E:\\Picked At The Deadline')
                return ('deadline_exceeded', 'the dialog outlived the deadline')
            if self.grant_mode == 'deadline_late':
                threading.Timer(0.8, self.land, ['E:\\Picked After The Deadline']).start()
                return ('deadline_exceeded', 'the dialog outlived the deadline')
            self.land('E:\\Picked Folder')
            return {'outcome': 'granted', 'root': self.roots[-1], 'refusal': None}
        if self.process_error is not None:
            if cmd == 'processes.explain':
                self.explain_calls.append(p['process_ref'])
                if self.explain_partial_once:
                    self.explain_partial_once = False
                    return ('partial_coverage', 'synthetic process sample not ready')
                result = json.loads((ROOT / 'contracts/v3/examples/commands/processes.explain.result.json').read_text())
                result['process_ref'] = p['process_ref']
                return result
            if cmd in ('telemetry.subscribe', 'telemetry.unsubscribe'):
                self.telemetry_calls.append(req)
                if cmd == 'telemetry.unsubscribe':
                    return {'subscription_id': p['subscription_id'], 'stopped': True}
                result = json.loads((ROOT / 'contracts/v3/examples/commands/telemetry.subscribe.result.json').read_text())
                result['subscription_id'] = f'sub_fake_{len(self.telemetry_calls)}'
                return result
            if cmd in ('telemetry.snapshot', 'processes.list', 'tree.slice', 'budgets.get'):
                if cmd == 'processes.list' and self.process_error:
                    return (self.process_error, 'synthetic process observation failure')
                result = json.loads((ROOT / f'contracts/v3/examples/commands/{cmd}.result.json').read_text())
                if cmd == 'processes.list' and self.trend_mode:
                    result['rows'][0]['process_ref'] = 'pc_4242_116444736000000001'
                if cmd == 'telemetry.snapshot' and self.trend_mode:
                    if self.trend_mode == 'refused':
                        return ('partial_coverage', 'synthetic telemetry gap')
                    if self.trend_mode == 'missing':
                        result['trends'] = {'series': [], 'candidates': [], 'candidates_truncated': False, 'unknowns': ['Missing samples and denied handles: coverage unknown.']}
                    else:
                        result['trends']['series'].append({
                            'key': 'system', 'metric': 'cpu_busy_fraction', 'unit': 'fraction', 'max_gap_ms': 2000,
                            'points': [{'offset_ms': t, 'observed_at': f'2026-10-09T12:00:{i:02}Z', 'value': v} for i, (t, v) in enumerate(zip((0, 1000, 2000, 9000, 10000), (0, 0.4, None, 0.8, 1)))],
                            'unknowns': ['Missing samples and sampling gaps remain gaps, not zero.'],
                        })
                if cmd == 'processes.list' and self.process_observed_at:
                    result['observed_at'] = self.process_observed_at
                    for row in result['rows']:
                        row['observed_at'] = self.process_observed_at
                return result
        if cmd == 'tree.slice':
            delay, root = self.slice_queue.pop(0) if self.slice_queue else (0.0, 'Fake Root')
            time.sleep(delay)
            return {'anchor_node_id': 'nd_atlas', 'basis': 'logical', 'root_generations': [], 'complete': True, 'live': False, 'ordering': 'exact', 'truncated': False,
                    'nodes': [{'node_id': 'nd_atlas', 'parent': None, 'name': 'Atlas'}, {'node_id': 'nd_' + self.node_prefix + root.replace(' ', '_'), 'parent': 0, 'name': root}]}
        if cmd == 'tree.children':
            if p['node_id'] in self.gone:
                return ('not_found', 'fake engine: no such node')
            if p['node_id'] in self.broken:
                return (self.broken[p['node_id']], 'fake engine: ancestor failed')
            if self.children_gate and p['node_id'] == self.children_gate[0]:
                _, arrived, release = self.children_gate
                self.children_gate = None
                arrived.set()
                if not release.wait(9):
                    return ('deadline_exceeded', 'synthetic children gate expired')
            if self.nested:
                prefix = f'nd_{self.node_prefix}dir_'
                depth = int(p['node_id'].removeprefix(prefix)) if p['node_id'].startswith(prefix) else 0
                sub = [{**row(f'dir-{depth + 1}', 0), 'node_id': f'{prefix}{depth + 1}', 'kind': 'dir', 'extension': None, 'ext_family': None}] if depth < self.nested_depth else []
                return {'anchor': p['node_id'], 'generation': '7', 'items': sub + [row('leaf.txt', 1)], 'next_cursor': None, 'total': len(sub) + 1, 'budget_hit': False}
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
            return {'volumes': self.tier_volumes if self.tier_volumes is not None else [{'volume_id': 'vo_x', 'display_name': 'X:', 'tier': tier, 'online': True, 'writable': True, 'capacity_bytes': '1000000000000', 'free_bytes': '90000000000', 'reserve_bytes': '1', 'free_fraction': 0.09, 'pressure': 'pressure', 'source_eligible': True}],
                    'policy': {'watch_free_fraction': 0.2, 'pressure_free_fraction': 0.1, 'reserve_note': 'r', 'note': 'n'}}
        if cmd == 'placement.candidates':
            self.candidate_sources.append(p['source_volume_id'])
            g = {'group_id': 'cg_a', 'node_id': 'nd_a', 'root_id': 'rt_a', 'name': 'group-a', 'source_bytes': '5000', 'destination_bytes': '5000', 'transfer_bytes': '5000', 'estimate_basis': 'allocated_entries',
                 'estimated_relief_bytes': '5000', 'relief_basis': 'verified_unique_allocation', 'heat': None, 'heat_basis': 'unknown', 'newest_modified_at': None, 'pinned': False, 'active': False, 'protected': False, 'days_since_move': 30, 'coverage': 'complete'}
            groups = self.placement_groups.get(p['source_volume_id'], [g])
            gate = self.candidate_gate
            # A gate naming a source waits for that source's request; an earlier request for another
            # volume still in flight (slow runners) must not consume it.
            if gate and (len(gate) < 3 or gate[2] == p['source_volume_id']):
                self.candidate_gate = None
            else:
                gate = None
            if gate:
                gate[0].set()
                if not gate[1].wait(60):  # every user releases in finally; slow CI runners need the headroom
                    return ('deadline_exceeded', 'synthetic candidate gate expired')
            return {'source_volume_id': p['source_volume_id'], 'root_generations': [], 'groups': groups, 'excluded_volumes': [], 'note': 'n'}
        if cmd == 'volumes.declare_tier':
            return {'volume': json.loads((ROOT / 'contracts/v3/examples/commands/volumes.declare_tier.result.json').read_text())['volume'], 'preference_recorded_at': '2026-10-01T00:00:00Z'}
        if cmd == 'proposals.get':
            detail = json.loads((ROOT / 'contracts/v3/examples/commands/proposals.get.result.json').read_text())
            detail['summary']['source_volume_id'] = self.saved_source
            return detail
        if cmd == 'proposals.list':
            if self.fail_proposals:
                return ('internal_error', 'synthetic saved scenarios failure')
            return {'proposals': [self.result({'command': 'proposals.get'})['summary']] if self.saved_scenarios else []}
        if cmd == 'placement.simulate':
            self.simulate_overrides.append(p.get('overrides'))
            plan = {'mode': 'simulation', 'algorithm': 'bounded_portfolio_search_v2', 'optimality_claim': False, 'optimality_scope': 's', 'shortfall_optimal': True,
                    'search': {'complete': True, 'nodes_visited': 1, 'node_budget': 10, 'reason': 'exhausted', 'eligible_groups': 1, 'eligible_targets': 1, 'lower_bound_shortfall_bytes': '0'},
                    'proposals': [{'group_id': 'cg_a', 'source_id': 'vo_x', 'target_id': 'vo_y', 'source_bytes_relieved': '5000', 'destination_bytes_required': '5000', 'transfer_bytes': '5000', 'reason': 'bounded_capacity_budget_allocation', 'requires_consent': True, 'executable': True}],
                    'rejected': [], 'pre_rejected': [], 'excluded_volumes': [], 'relief_policy': 'verified_only', 'projected_free_bytes': {}, 'target_free_bytes': '1', 'shortfall_bytes': '0', 'satisfied': True,
                    'transfer_bytes': '5000', 'filesystem_changed': False, 'baseline_shortfall_bytes': '777', 'shortfall_improvement_bytes': '777', 'assumption': 'a', 'heat_policy': p['heat_policy'],
                    'assumptions': [], 'root_generations': [], 'proposal_id': None}
            if self.plan_mode == 'malformed':
                plan['proposals'] = 'none'
            elif self.plan_mode == 'safe':
                allowed = {g['group_id'] for g in self.result({'command': 'placement.candidates', 'payload': p})['groups']}
                if any(o['group_id'] not in allowed for o in p.get('overrides', [])):
                    return ('invalid_request', 'duplicate_or_unknown_group_override')
                plan['proposals'][0]['group_id'] = next(iter(allowed))
                plan['proposals'][0]['executable'] = False
            return plan
        if cmd == 'search.query':
            text = p.get('text', '')
            if text == 'slow':
                time.sleep(1.5)
            name = 'SLOW-RESULT.txt' if text == 'slow' else 'FAST-RESULT.txt'
            generation, cursor, budget = '7', None, False
            if text == 'paged':  # the root's generation moves between the two pages (a watcher run)
                second = p.get('cursor') == 'c_page2'
                name, generation, cursor = ('PAGED-TWO.txt', '8', None) if second else ('PAGED-ONE.txt', '7', 'c_page2')
            hit = {'row': {**row(name, 2 if name == 'PAGED-TWO.txt' else 1), 'location_hint': {'text': 'Fake Root', 'truncated': False}}, 'root_id': 'rt_fake', 'path': {'text': name, 'truncated': False},
                   'match_kind': 'substring', 'highlights': [[0, 4]]}
            hits = [hit]
            if text == 'cut':  # the page ran out of budget before any match
                hits, cursor, budget = [], 'c_cut', True
            return {'generation': generation, 'hits': hits, 'next_cursor': cursor, 'budget_hit': budget, 'index_state': 'building', 'partial': True, 'substring_searched': True,
                    'roots': [{'root_id': 'rt_fake', 'scan_state': 'scanning', 'coverage': 'partial'}]}
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
    first = lambda: rows(page).first.locator('th').first.inner_text().strip()  # the copy checkbox precedes the name
    by_size = first()
    page.get_by_label('Sort').select_option('name_asc')
    expect(rows(page).first.locator('th').first).not_to_have_text(by_size)
    ok(first().startswith('"><svg/onload=alert(1)>'), 'name sort puts the quote-leading name first')
    page.get_by_label('Sort').select_option('size_desc')
    expect(rows(page).first.locator('th').first).to_have_text(by_size)
    print('PASS sort by name and back to size')

    # basis
    page.get_by_role('button', name='Allocated', exact=True).click()
    expect(page.get_by_role('button', name='Allocated', exact=True)).to_have_attribute('aria-pressed', 'true')
    expect(page.locator('th.basis')).to_have_text('Allocated')
    page.get_by_role('button', name='Logical', exact=True).click()
    expect(page.locator('th.basis')).to_have_text('Logical')
    print('PASS size basis toggles and marks its column')

    # search filters
    page.get_by_placeholder('Search names').fill('delta')
    page.get_by_label('Kind').select_option('dir')
    page.get_by_role('button', name='Search', exact=True).click()
    expect(page.get_by_text('Search results for')).to_be_visible()
    expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
    kinds = rows(page).locator('td:nth-child(2)').all_inner_texts()
    ok(len(kinds) > 0 and set(kinds) == {'dir'}, 'kind filter keeps folders only')
    # tiers, highlights and the keyboard: Down from the box walks the Inspect buttons, Escape returns
    marks = rows(page).locator('th mark').all_inner_texts()
    ok(len(marks) > 0 and all(m.lower() == 'delta' for m in marks), 'search highlights mark the matched text')
    ok(rows(page).first.locator('td').nth(5).inner_text().startswith('prefix'), 'each hit names its match tier')
    box = page.get_by_placeholder('Search names')
    box.focus()
    box.press('ArrowDown')
    first = rows(page).nth(0).get_by_role('button', name='Inspect')
    expect(first).to_be_focused()
    first.press('ArrowDown')
    expect(rows(page).nth(1).get_by_role('button', name='Inspect')).to_be_focused()
    page.keyboard.press('Escape')
    expect(box).to_be_focused()
    page.get_by_placeholder('Search names').fill('d')
    page.get_by_role('button', name='Search', exact=True).click()
    expect(page.get_by_role('list', name='Search coverage')).to_contain_text('one character searches whole names (or the name before a dot); two also search name and word starts.')
    print('PASS search tiers, highlights, short-text notice and keyboard walk')
    page.get_by_label('Kind').select_option('any')
    page.get_by_placeholder('Search names').fill('')
    page.get_by_label('Extension').fill('pdf')
    page.get_by_role('button', name='Search', exact=True).click()
    expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
    names = rows(page).locator('th:first-child').all_inner_texts()
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
    coverage = page.get_by_role('list', name='Search coverage')
    expect(coverage).to_contain_text('still being scanned')
    expect(coverage).to_contain_text('not proof of absence')
    page.wait_for_timeout(2000)
    ok(page.get_by_text('SLOW-RESULT.txt').count() == 0 and page.get_by_text('FAST-RESULT.txt').count() == 1, 'a slow reply for an older search is dropped')
    # a root generation that moves between pages (every watcher run) does not restart a search
    box.fill('paged')
    page.get_by_role('button', name='Search', exact=True).click()
    expect(page.get_by_text('PAGED-ONE.txt')).to_be_visible()
    page.get_by_role('button', name='Load more').click()
    expect(page.get_by_text('PAGED-TWO.txt')).to_be_visible()
    ok(page.get_by_text('PAGED-ONE.txt').count() == 1 and page.get_by_text('starts again from the top').count() == 0,
       'search paging continues across a root generation change')
    box.fill('cut')
    page.get_by_role('button', name='Search', exact=True).click()
    expect(page.locator('main table tbody')).to_contain_text('Nothing found yet')
    print('PASS an empty budget-cut search page reads as unfinished, not as no matches')
    page.get_by_role('button', name='Clear search').click()
    expect(rows(page)).to_have_count(50)

    # a cursor that went stale restarts the listing from the top, with a notice
    notice_h = page.locator('#explorer-notice').element_handle()  # 4.1.3: a persistent region, empty until it speaks
    ok(notice_h.text_content() == '' and notice_h.get_attribute('role') == 'status', 'the Explorer notice region is in the page, empty, before it has anything to say')
    engine.stale_once = True
    page.get_by_role('button', name='Load more').click()
    expect(page.get_by_text('starts again from the top')).to_be_visible()
    ok(notice_h.evaluate('e => e.isConnected && e.textContent.includes("starts again")'), 'the restart notice is spoken by the region that was already there')
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


def check_explorer_fallback(page: Page, engine: FakeEngine) -> None:
    # #198: a saved breadcrumb whose node vanished keeps the reader at the deepest ancestor that still lists
    crumbs = page.locator('main nav[aria-label="Location"]')
    notice = page.locator('#explorer-notice')
    engine.nested = True
    page.goto(f'{engine.base}/#token={TOKEN}')
    page.evaluate("location.hash = '#/explorer'")
    expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
    for n in (1, 2, 3):
        page.locator('main table.tbl').get_by_role('button', name=f'dir-{n}', exact=True).click()
        expect(crumbs).to_contain_text(f'dir-{n}')
        expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')  # listed, so it is a saved breadcrumb
    engine.gone = {'nd_dir_2', 'nd_dir_3'}
    current = crumbs.get_by_role('button', name='dir-3', exact=True)
    current.focus()
    page.keyboard.press('Shift+Tab')
    page.keyboard.press('Tab')
    expect(current).to_be_focused()
    engine.invalidate()
    expect(notice).to_contain_text('dir-3 is no longer in the catalogue')
    ok(crumbs.inner_text().split() == ['Fake', 'Root', '›', 'dir-1'], f'the view climbed to the deepest folder that still lists, not the top level: {crumbs.inner_text()!r}')
    ok('moved up to dir-1' in notice.text_content() and notice.get_attribute('role') == 'status', 'the notice names the lost folder and the folder shown, in the existing status region')
    expect(rows(page)).to_have_count(2)
    expect(crumbs.get_by_role('button', name='dir-1', exact=True)).to_be_focused()
    page.keyboard.press('Tab')
    expect(page.get_by_role('checkbox', name='Select dir-2 for copy')).to_be_focused()
    # the next action the reader takes clears the notice
    engine.gone = set()
    page.locator('main table.tbl').get_by_role('button', name='dir-2', exact=True).click()
    expect(notice).to_have_text('')
    expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
    # The focused row can vanish with its containing folder before it is activated.
    folder = page.locator('main table.tbl').get_by_role('button', name='dir-3', exact=True)
    folder.focus()
    page.keyboard.press('Shift+Tab')
    page.keyboard.press('Tab')
    expect(folder).to_be_focused()
    engine.gone = {'nd_dir_2', 'nd_dir_3'}
    engine.invalidate()
    expect(notice).to_contain_text('dir-2 is no longer in the catalogue')
    expect(crumbs.get_by_role('button', name='dir-1', exact=True)).to_be_focused()
    page.keyboard.press('Tab')
    expect(page.get_by_role('checkbox', name='Select dir-2 for copy')).to_be_focused()
    engine.gone = set()
    page.locator('main table.tbl').get_by_role('button', name='dir-2', exact=True).click()
    page.locator('main table.tbl').get_by_role('button', name='dir-3', exact=True).click()
    expect(crumbs).to_contain_text('dir-3')
    expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
    # the ancestor it climbs to fails too: the error shows, with no rows and no claim that the view moved
    engine.gone = {'nd_dir_3'}
    engine.broken = {'nd_dir_2': 'internal_error'}
    engine.invalidate()
    expect(page.get_by_role('alert').filter(has_text='ancestor failed')).to_be_visible()
    ok(notice.text_content() == '', f'a failed ancestor listing publishes no "moved up" notice: {notice.text_content()!r}')
    ok(page.get_by_text('leaf.txt').count() == 0, 'a failed ancestor listing leaves no rows on screen')
    engine.gone = set()
    engine.broken = {}
    engine.invalidate()
    expect(rows(page)).to_have_count(2)
    page.locator('main table.tbl').get_by_role('button', name='dir-3', exact=True).click()
    expect(crumbs).to_contain_text('dir-3')
    expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
    # no ancestor lists any more (an engine restart): the view returns to the starting points, and says so
    engine.gone = {'nd_Fake_Root', 'nd_dir_1', 'nd_dir_2', 'nd_dir_3'}
    # The epoch refresh and breadcrumb recovery both read the replacement starting points.
    engine.slice_queue = [(0.0, 'New Root'), (0.0, 'New Root')]
    current.focus()
    page.keyboard.press('Shift+Tab')
    page.keyboard.press('Tab')
    engine.invalidate()
    expect(notice).to_contain_text('dir-3 is no longer in the catalogue')
    expect(notice).to_contain_text('went back to the top level')
    ok(crumbs.inner_text().split() == ['New', 'Root'], f'with the whole saved chain gone the view shows the new starting point: {crumbs.inner_text()!r}')
    expect(crumbs.get_by_role('button', name='New Root')).to_be_focused()
    page.keyboard.press('Tab')
    expect(page.get_by_role('checkbox', name='Select dir-1 for copy')).to_be_focused()
    engine.gone = set()
    engine.nested = False
    print('PASS a vanished folder keeps the reader at the nearest folder that still exists, and says so')


def check_explorer_reissued_ids(page: Page, engine: FakeEngine) -> None:
    crumbs = page.locator('main nav[aria-label="Location"]')
    notice = page.locator('#explorer-notice')
    engine.nested = True
    page.goto(f'{engine.base}/#token={TOKEN}')
    page.evaluate("location.hash = '#/explorer'")
    for n in (1, 2, 3):
        folder = page.locator('main table.tbl').get_by_role('button', name=f'dir-{n}', exact=True)
        folder.focus()
        folder.press('Enter')
        expect(crumbs).to_contain_text(f'dir-{n}')
        expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
    engine.gone = {'nd_Fake_Root', 'nd_dir_1', 'nd_dir_2', 'nd_dir_3'}
    engine.node_prefix = 'new_'
    engine.nested_depth = 1
    crumbs.get_by_role('button', name='dir-3', exact=True).focus()
    engine.invalidate()
    expect(notice).to_contain_text('dir-3 is no longer in the catalogue')
    expect(notice).to_contain_text('moved up to dir-1')
    expect(crumbs.get_by_role('button')).to_have_count(2)
    expect(crumbs.get_by_role('button', name='dir-1', exact=True)).to_be_focused()
    print('PASS reissued node IDs resolve the old path to its deepest surviving ancestor')

    # A second rebuild keeps the whole path. Moving focus during recovery must be respected.
    engine.nested_depth = 3
    engine.invalidate()
    for n in (2, 3):
        page.locator('main table.tbl').get_by_role('button', name=f'dir-{n}', exact=True).click()
        expect(crumbs).to_contain_text(f'dir-{n}')
        expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
    for prefix in ('again_', 'later_'):
        engine.gone.update({f'nd_{engine.node_prefix}Fake_Root', *(f'nd_{engine.node_prefix}dir_{n}' for n in (1, 2, 3))})
        engine.node_prefix = prefix
        arrived, release = threading.Event(), threading.Event()
        engine.children_gate = (f'nd_{prefix}Fake_Root', arrived, release)
        crumbs.get_by_role('button', name='dir-3', exact=True).focus()
        engine.invalidate()
        try:
            wait_until(page, arrived.is_set)
            search = page.get_by_role('searchbox', name='Search names')
            search.focus()
            if prefix == 'later_':
                search.fill('fast')
                search.press('Enter')
                expect(page.get_by_text('FAST-RESULT.txt', exact=True)).to_be_visible()
        finally:
            release.set()
        if prefix == 'again_':
            expect(notice).to_contain_text('this folder is still available')
            expect(crumbs.get_by_role('button')).to_have_count(4)
            expect(crumbs.get_by_role('button', name='dir-3', exact=True)).to_have_attribute('aria-current', 'location')
        else:
            page.wait_for_timeout(200)
            expect(page.get_by_text('FAST-RESULT.txt', exact=True)).to_be_visible()
            expect(notice).to_have_text('')
            expect(crumbs).to_have_count(0)
        expect(search).to_be_focused()
    engine.node_prefix = ''
    engine.nested = False
    engine.gone = set()
    print('PASS restored paths stay honest; recovery respects moved focus and cannot replace a newer search')


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


def check_tiers_loading_followups(page: Page, engine: FakeEngine) -> None:
    engine.fail_proposals = True
    page.goto(f'{engine.base}/#token={TOKEN}')
    page.evaluate("location.hash = '#/tiers'")
    panel = page.locator('section[aria-labelledby="h-vol"]')
    saved = page.locator('section[aria-labelledby="h-saved"]')
    expect(saved.get_by_role('alert')).to_contain_text('synthetic saved scenarios failure')
    expect(panel.get_by_role('heading', name='X:', exact=True)).to_be_visible()
    expect(panel).to_have_attribute('aria-busy', 'false')
    ok(True, 'Tiers keeps volumes and scopes proposals.list errors to saved scenarios')

    engine.fail_proposals = False
    engine.saved_scenarios = True
    page.reload()
    button = saved.get_by_role('button', name=re.compile('Show saved scenario'))
    expect(button).to_be_visible()
    expect(panel).to_have_attribute('aria-busy', 'false')
    arrived, release = threading.Event(), threading.Event()
    engine.candidate_gate = (arrived, release)
    try:
        panel.get_by_label('Declared tier for X:').select_option('2')
        wait_until(page, arrived.is_set)
        expect(panel).to_have_attribute('aria-busy', 'true')
        button.click()
        expect(page.get_by_role('heading', name='Placement plan', level=2)).to_be_visible()  # durable: the live-region sentence can be replaced
    finally:
        release.set()
    expect(panel).to_have_attribute('aria-busy', 'false')
    expect(panel.get_by_role('heading', name='X:', exact=True)).to_be_visible()
    ok(True, 'concurrent saved scenario loading cannot strand the model loading flag')
    engine.saved_scenarios = False


# --- Tiers ------------------------------------------------------------------------------------------

def declare_tier(page: Page, engine: FakeEngine, panel, tier: str) -> None:
    """Declares a tier and returns only once the reload it triggers has been served and applied.

    `aria-busy` covers the reload but not the declare round trip before it, so on its own it reads
    'false' straight after the change and a later step can race the reload (which discards a
    scenario that is loading, `loadModel` bumps its ticket)."""
    expect(panel).to_have_attribute('aria-busy', 'false')
    before = {c: engine.served.get(c, 0) for c in ('volumes.declare_tier', 'proposals.list')}
    panel.get_by_label('Declared tier for X:').select_option(tier)
    wait_until(page, lambda: all(engine.served.get(c, 0) > n for c, n in before.items()))
    expect(panel).to_have_attribute('aria-busy', 'false')


def check_tiers_source_eligibility(page: Page, engine: FakeEngine) -> None:
    template = engine.result({'command': 'tiers.model'})['volumes'][0]
    engine.tier_volumes = [
        {**template, 'volume_id': 'vo_blocked', 'display_name': 'Blocked', 'source_eligible': False},
        {**template, 'pressure': 'ok'},
        {**template, 'volume_id': 'vo_other', 'display_name': 'Other', 'pressure': 'ok'},
    ]
    engine.candidate_sources.clear()
    page.goto(f'{engine.base}/#token={TOKEN}')
    page.evaluate("location.hash = '#/tiers'")
    panel = page.locator('section[aria-labelledby="h-vol"]')
    selected = panel.get_by_role('button', name=re.compile('^Source volume'))
    expect(selected).to_contain_text('X:')
    expect(panel.get_by_role('button', name=re.compile(r'^Use as source\s*:\s*Blocked$'))).to_be_disabled()
    panel.get_by_role('button', name=re.compile(r'^Use as source\s*:\s*Other$')).click()
    expect(selected).to_contain_text('Other')
    declare_tier(page, engine, panel, '2')
    expect(selected).to_contain_text('Other')
    engine.tier_volumes[2]['source_eligible'] = False
    declare_tier(page, engine, panel, '3')
    expect(selected).to_contain_text('X:')
    expect(panel.get_by_role('button', name=re.compile(r'^Use as source\s*:\s*Other$'))).to_be_disabled()
    engine.tier_volumes[1]['source_eligible'] = False
    engine.saved_scenarios = True
    declare_tier(page, engine, panel, '4')
    expect(selected).to_have_count(0)
    count = len(engine.candidate_sources)
    page.get_by_role('button', name=re.compile('Show saved scenario')).click()
    expect(page.get_by_role('heading', name='Placement plan', level=2)).to_be_visible()  # durable: the live-region sentence can be replaced
    expect(selected).to_have_count(0)
    ok(len(engine.candidate_sources) == count and 'vo_blocked' not in engine.candidate_sources,
       'Tiers defaults, preserves, enables and restores only eligible sources; ineligible saved sources stay historical')
    # Codex P2 on #340: a refresh that replaces an ineligible source clears that source's assumptions.
    engine.saved_scenarios = False
    engine.tier_volumes = [{**template, 'pressure': 'ok'}, {**template, 'volume_id': 'vo_other', 'display_name': 'Other', 'pressure': 'ok'}]
    page.goto(f'{engine.base}/#token={TOKEN}')
    page.evaluate("location.hash = '#/tiers'")
    expect(selected).to_contain_text('X:')
    page.get_by_label('Assumed heat for group-a').select_option('0.9')
    engine.tier_volumes[0]['source_eligible'] = False
    declare_tier(page, engine, panel, '5')
    expect(selected).to_contain_text('Other')
    expect(panel).to_have_attribute('aria-busy', 'false')
    engine.simulate_overrides.clear()
    page.get_by_role('button', name='Simulate', exact=True).click()
    expect(page.get_by_role('heading', name='Alternatives')).to_be_visible()
    ok(engine.simulate_overrides and engine.simulate_overrides[-1] == [],
       f'a source replaced on refresh sends no stale group overrides ({engine.simulate_overrides[-1:] if engine.simulate_overrides else None})')
    engine.tier_volumes = None


def check_tiers_assumption_refresh(page: Page, engine: FakeEngine) -> None:
    template = engine.result({'command': 'tiers.model'})['volumes'][0]
    a = engine.result({'command': 'placement.candidates', 'payload': {'source_volume_id': 'vo_x'}})['groups'][0]
    b = {**a, 'group_id': 'cg_b', 'node_id': 'nd_b', 'name': 'group-b'}
    engine.tier_volumes = [template, {**template, 'volume_id': 'vo_other', 'display_name': 'Other'}]
    engine.placement_groups = {'vo_x': [a, b], 'vo_other': [a]}
    engine.saved_scenarios = True
    engine.plan_mode = 'safe'
    page.goto(f'{engine.base}/#token={TOKEN}')
    page.evaluate("location.hash = '#/tiers'")
    panel = page.locator('section[aria-labelledby="h-vol"]')

    def simulate(expected):
        engine.simulate_overrides.clear()
        page.get_by_role('button', name='Simulate', exact=True).click()
        expect(page.get_by_role('heading', name='Alternatives')).to_be_visible()
        expect(page.get_by_role('heading', name='Placement plan', level=2)).to_be_visible()
        ok(len(engine.simulate_overrides) == 3 and all(o == expected for o in engine.simulate_overrides),
           f'Tiers sends only current assumptions and all simulations succeed: {expected}')

    release = None
    try:
        page.get_by_label('Assumed heat for group-a').select_option('0.9')
        page.get_by_label('Assumed heat for group-b').select_option('0.1')
        engine.placement_groups['vo_x'] = [b]
        declare_tier(page, engine, panel, '2')
        expect(page.get_by_label('Assumed heat for group-a')).to_have_count(0)
        expect(page.get_by_label('Assumed heat for group-b')).to_have_value('0.1')
        simulate([{'group_id': 'cg_b', 'heat': 0.1}])
        # A vanished group's assumption must not return if its ID reappears later.
        engine.placement_groups['vo_x'] = [a, b]
        declare_tier(page, engine, panel, '3')
        expect(page.get_by_label('Assumed heat for group-a')).to_have_value('')
        page.get_by_label('Assumed heat for group-a').select_option('0.9')
        engine.saved_source = 'vo_other'
        page.get_by_role('button', name=re.compile('Show saved scenario')).click()
        expect(panel.get_by_role('button', name=re.compile('^Source volume'))).to_contain_text('Other')
        expect(page.get_by_label('Assumed heat for group-a')).to_have_value('')
        simulate([])  # Even an ID shared across sources must lose the old assumption.

        # Release an older source's response after the new candidates have been installed.
        arrived, release = threading.Event(), threading.Event()
        engine.candidate_gate = (arrived, release, 'vo_x')
        page.get_by_role('button', name=re.compile(r'^Use as source\s*:\s*X:$')).click()
        wait_until(page, arrived.is_set)
        page.get_by_role('button', name=re.compile(r'^Use as source\s*:\s*Other$')).click()
        expect(page.get_by_label('Assumed heat for group-a')).to_be_visible()
        page.get_by_label('Assumed heat for group-a').select_option('0.5')
        with page.expect_request_finished(lambda r: r.method == 'POST' and
                                          r.post_data_json.get('command') == 'placement.candidates' and
                                          r.post_data_json['payload']['source_volume_id'] == 'vo_x'):
            release.set()
        page.evaluate('() => new Promise(requestAnimationFrame)')
        expect(page.get_by_label('Assumed heat for group-b')).to_have_count(0)
        expect(page.get_by_label('Assumed heat for group-a')).to_have_value('0.5')
        simulate([{'group_id': 'cg_a', 'heat': 0.5}])
    finally:
        if release:
            release.set()
        engine.placement_groups = {}
        engine.tier_volumes = None
        engine.saved_source = 'vo_x'
        engine.saved_scenarios = False
        engine.plan_mode = 'executable'


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
    # 19% of the generic 2 TiB source (389 GiB) is within 184 GiB free plus 212 GiB relief: the target-met path
    page.get_by_label('Free space wanted on the source').fill('19')
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


# --- Backups ----------------------------------------------------------------------------------------

def check_backups(page: Page, base: str, shoot) -> None:
    page.goto(f'{base}/?transport=mock#/backups')
    expect(page.get_by_role('heading', name='Backups', level=1)).to_be_visible()
    main = page.locator('main')
    expect(main.get_by_text('A green backup job is not verified recovery.', exact=True)).to_be_visible()
    for repo in ('synthetic-nas', 'synthetic-usb'):
        expect(page.get_by_role('heading', name=repo, level=2)).to_be_visible()
    nas = main.locator('section').filter(has=page.get_by_role('heading', name='synthetic-nas'))
    # three separate observations, each with its own refusals in plain words
    table = nas.get_by_role('table', name='Recovery evidence for synthetic-nas')
    ok(table.locator('tbody th').all_inner_texts() == ['Snapshot taken', 'Integrity checked', 'Restore drill'], 'snapshot, integrity and restore drill are separate rows')
    expect(table.get_by_role('row', name=re.compile('^Integrity checked'))).to_contain_text('Structure only, or full data read not shown')
    expect(table.get_by_role('row', name=re.compile('^Integrity checked'))).to_contain_text('No clean integrity check that read all the data.')
    expect(table.get_by_role('row', name=re.compile('^Restore drill'))).to_contain_text('No restore drill has been observed.')
    expect(nas.get_by_text('Recovery not shown', exact=True)).to_be_visible()
    expect(nas.get_by_text('0 of 2 needed versions covered by a restore')).to_be_visible()
    expect(nas.get_by_text('cannot yet tell whether they share a physical disk')).to_be_visible()
    expect(main.locator('section').filter(has=page.get_by_role('heading', name='synthetic-usb')).get_by_text('An observation is older than the freshness window.')).to_be_visible()
    # the proposed drill is data: escaped names, no control to run it
    nas.locator('details.drill summary').click()
    expect(nas.get_by_text('U+202E RLO', exact=True)).to_be_visible()
    ok(CONTROL_CHARS.search(main.inner_text()) is None, 'backups: a bidi control in a needed file name is shown as a badge, never raw')
    expect(nas.get_by_text('Loomward does not run this drill.')).to_be_visible()
    labels = [b.inner_text() for b in main.get_by_role('button').all()]
    ok(not any(FORBIDDEN_BUTTONS.search(t) for t in labels), f'backups: no effect control among {labels}')
    expect(main.get_by_text('Importing needs the desktop app in a personal session')).to_be_visible()
    shoot('backups')
    page.get_by_role('button', name='Forget the evidence for synthetic-usb').click()
    expect(page.get_by_role('status', name='Backup result')).to_have_text('Forgot the evidence for synthetic-usb. No backup or file was changed.')
    expect(page.get_by_role('heading', name='synthetic-usb')).to_have_count(0)
    expect(page.get_by_role('heading', name='Backups', level=1)).to_be_focused()
    print('PASS Backups: separate observations with plain refusals, honest coverage and topology, drill as data, forget')


def check_alerts(page: Page, base: str, shoot) -> None:
    page.goto(f'{base}/?transport=mock#/alerts')
    expect(page.get_by_role('heading', name='Alerts', level=1)).to_be_visible()
    main = page.locator('main')
    expect(main.get_by_text('Unknown is not zero:')).to_be_visible()
    expect(page.get_by_role('heading', name='Active alerts (1)', level=2)).to_be_visible()
    table = main.get_by_role('table', name='Low-space and growth checks with their coverage')
    ok(table.locator('tbody th').count() == 3, 'one row per check, the unobserved volume and the partial root included')
    # an unobserved volume is listed as such: no zero free, no alert
    unseen = table.get_by_role('row', name=re.compile('^Synthetic volume B'))
    expect(unseen).to_contain_text('Not observed')
    expect(unseen).to_contain_text('unknown')
    # a partial scan qualifies the growth check instead of reading as a number
    partial = table.get_by_role('row', name=re.compile('^Synthetic root 1'))
    expect(partial).to_contain_text('Partial coverage')
    ok('+' not in partial.inner_text(), 'a partial-coverage check shows no growth figure')
    ok(not any(FORBIDDEN_BUTTONS.search(b.inner_text()) for b in main.get_by_role('button').all()), 'alerts: no effect control')
    notify = main.get_by_label('Notify me when notifications arrive (stored now; no notification is sent yet)')
    expect(notify).not_to_be_checked()
    shoot('alerts')
    # a bad threshold is named on its field and nothing is saved
    main.get_by_label('Low space: percent of capacity').fill('0')
    page.get_by_role('button', name='Save thresholds').click()
    expect(page.get_by_role('alert').filter(has_text='whole percentage')).to_be_visible()
    expect(main.get_by_label('Low space: percent of capacity')).to_have_attribute('aria-invalid', 'true')
    main.get_by_label('Low space: percent of capacity').fill('15')
    notify.check()
    page.get_by_role('button', name='Save thresholds').click()
    expect(page.get_by_role('status', name='Alert settings result')).to_contain_text('Thresholds saved.')
    expect(main.get_by_label('Low space: percent of capacity')).to_have_value('15')
    expect(main.get_by_text('Notifications are enabled, but no operating-system notification channel is wired yet')).to_be_visible()
    page.get_by_role('button', name='Fill in defaults').click()
    expect(main.get_by_label('Low space: percent of capacity')).to_have_value('10')
    expect(notify).not_to_be_checked()
    print('PASS Alerts: unknown and partial coverage are listed, never zero; thresholds validated and saved; notifications off by default')


def check_backups_import(page: Page, engine: FakeEngine) -> None:
    fixture = json.loads((ROOT / 'contracts/v3/examples/commands/backups.import.result.json').read_text(encoding='utf-8'))
    imported = {**fixture, 'evidence': {**fixture['evidence'], 'evidence_id': 'be_9', 'repository_label': 'synthetic-new'}}
    engine.backups = []
    engine.backup_answers = [
        {'outcome': 'cancelled_by_user', 'evidence': None, 'refusal': None},
        {'outcome': 'refused', 'evidence': None, 'refusal': 'too_large'},
        imported,
    ]
    try:
        page.goto('about:blank')  # a new document, so session.hello runs again against the desktop-like engine
        page.goto(f'{engine.base}/#token={TOKEN}')
        page.evaluate("location.hash = '#/backups'")
        expect(page.get_by_role('heading', name='Backups', level=1)).to_be_visible()
        expect(page.get_by_text('No backup evidence imported yet.')).to_be_visible()
        choose = page.get_by_role('button', name='Choose receipt file')
        page.get_by_label('Repository label').fill('s3:https://bucket')
        choose.click()
        expect(page.get_by_role('alert')).to_have_text('Use a label of 1–64 letters, digits, dots, dashes or underscores.')
        # 3.3.1: the field in error is marked invalid and described by the message, by id
        label_field = page.get_by_label('Repository label')
        expect(label_field).to_have_attribute('aria-invalid', 'true')
        expect(label_field).to_have_attribute('aria-describedby', 'backup-error')
        expect(page.locator('#backup-error')).to_have_text('Use a label of 1–64 letters, digits, dots, dashes or underscores.')
        ok(engine.backup_calls == [], 'an invalid label never reaches the engine')
        label_field.fill('synthetic-new')
        page.get_by_label('Snapshot ID').fill('1' * 64)
        receipt_field = page.get_by_label('Files you need back')
        receipt_field.fill('not a receipt line')
        choose.click()
        expect(page.get_by_role('alert')).to_contain_text('Line 1: expected a relative name')
        expect(receipt_field).to_have_attribute('aria-invalid', 'true')
        expect(receipt_field).to_have_attribute('aria-describedby', 'backup-error')
        expect(label_field).not_to_have_attribute('aria-invalid', 'true')
        ok(engine.backup_calls == [], 'a malformed receipt line never reaches the engine')
        receipt_field.fill('fixture/ledger.ods ' + 'A' * 64)
        choose.click()
        expect(page.get_by_role('status', name='Backup result')).to_have_text('Import cancelled. Nothing was stored.')
        expect(receipt_field).not_to_have_attribute('aria-invalid', 'true')
        choose.click()
        expect(page.get_by_role('alert')).to_contain_text('larger than 256 KiB')
        choose.click()
        expect(page.get_by_role('status', name='Backup result')).to_contain_text('Evidence imported.')
        expect(page.get_by_role('heading', name='synthetic-new', level=2)).to_be_visible()
        call = engine.backup_calls[0]
        ok(call['deadline_ms'] == 60000, 'the import waits up to 60 s for the owner at the native dialog')
        ok(call['payload'] == {'repository_label': 'synthetic-new', 'snapshot_id': '1' * 64, 'source_volume_id': None, 'target_volume_id': None,
                               'required': [{'relative_name': 'fixture/ledger.ods', 'version': 'sha256:' + 'a' * 64}], 'integrity_check': True},
           f'the request carries the form and no path: {call["payload"]}')
        print('PASS Backups import: form validation, cancel, named refusal and import through the engine; no path leaves the page')
    finally:
        engine.backups = None


def check_diagnostics_save(page: Page, engine: FakeEngine) -> None:
    """The desktop-like engine offers the save: only the digest of what is shown leaves the page."""
    fixture = json.loads((ROOT / 'contracts/v3/examples/commands/diagnostics.preview.result.json').read_text(encoding='utf-8'))
    engine.diagnostics_answers = [
        {'outcome': 'cancelled_by_user', 'bytes_written': None, 'refusal': None},
        {'outcome': 'refused', 'bytes_written': None, 'refusal': 'preview_changed'},
        {'outcome': 'saved', 'bytes_written': fixture['bytes'], 'refusal': None, 'delay': 1.5},
        ('deadline_exceeded', 'the dialog outlived the deadline'),
        {'outcome': 'saved', 'bytes_written': fixture['bytes'], 'refusal': None},
    ]
    try:
        page.goto('about:blank')
        page.goto(f'{engine.base}/#token={TOKEN}')
        page.evaluate("location.hash = '#/health'")
        panel = page.locator('section[aria-labelledby="h-diag"]')
        expect(panel.get_by_text('Nothing is uploaded. Saving writes exactly what is shown to a file you choose.')).to_be_visible()
        expect(panel.get_by_text('the text below is exactly these bytes')).to_be_visible()
        expect(panel.get_by_role('region', name='Diagnostic bundle preview')).to_contain_text('"recent_errors"')
        save = panel.get_by_role('button', name='Save…')
        save.click()
        result = panel.get_by_role('status', name='Diagnostic bundle result')
        expect(result).to_have_text('Save cancelled. No file was written.')
        save.click()
        expect(panel.get_by_role('alert')).to_contain_text('The bundle changed since this preview')
        previews = engine.served.get('diagnostics.preview', 0)
        save.click()
        panel.get_by_role('button', name='Preview again').click(force=True)  # asked while the dialog is open
        page.wait_for_timeout(500)
        ok(engine.served.get('diagnostics.preview', 0) == previews, 'no new preview replaces the text while it is being saved')
        expect(result).to_contain_text(f"Saved {int(fixture['bytes']):,} bytes with digest {fixture['digest'][:19]}…, exactly the text shown.")
        wait_until(page, lambda: engine.served.get('diagnostics.preview', 0) == previews + 1, timeout_ms=5000)
        ok(True, 'the deferred preview runs once the save has answered')
        save.click()
        expect(panel.get_by_role('alert')).to_contain_text('The save is still pending in the dialog')
        save.click()  # asks for that same save again, never a second one
        expect(result).to_contain_text('Saved')
        ids = [c['request_id'] for c in engine.diagnostics_calls]
        ok(ids[3] == ids[4] and len(set(ids[:4])) == 4, f'the retried save reuses its request id: {ids}')
        for call in engine.diagnostics_calls:
            ok(call['payload'] == {'digest': fixture['digest']}, f'only the shown digest leaves the page: {call["payload"]}')
            ok(call['deadline_ms'] == 60000, 'the save waits up to 60 s for the owner at the native dialog')
        ok(len(engine.diagnostics_calls) == 5, 'one call per click')
        print('PASS diagnostic bundle save: cancel, named refusal, save and an outlived dialog kept pending and retried under the same request id; only the digest leaves the page')
    finally:
        engine.diagnostics_answers = None


# --- Companion --------------------------------------------------------------------------------------

def check_wait_until_dispatches_routes(browser) -> None:
    """Python predicates must pump Playwright so routed requests can complete."""
    engine = FakeEngine()
    engine.start()
    page = browser.new_page()
    seen = []
    try:
        page.goto(engine.base)
        def reply(route):
            seen.append(True)
            route.fulfill(status=200, content_type='text/plain', body='ok')
        url = engine.base + '/poll-probe'
        page.route(url, reply)
        page.evaluate("url => { setTimeout(() => { void fetch(url); }, 100); }", url)
        wait_until(page, lambda: bool(seen), timeout_ms=1500)
        ok(len(seen) == 1, 'Python predicate polling dispatches routed requests')
    finally:
        page.close()
        engine.stop()


def check_resource_trends(browser) -> None:
    check_wait_until_dispatches_routes(browser)
    engine = FakeEngine()
    engine.process_error = ''
    engine.trend_mode = 'gap'
    engine.start()
    page = browser.new_page()
    errors = []
    page.on('pageerror', lambda e: errors.append(str(e)))
    try:
        page.goto(f'{engine.base}/?transport=http#token={TOKEN}&/observatory')
        page.evaluate("location.hash = '#/observatory'")
        panel = page.get_by_role('region', name='Recent trends')
        expect(panel).to_be_visible()
        expect(panel.get_by_text('growth candidate', exact=True)).to_be_visible()
        expect(panel.locator('.explanations')).to_contain_text('synthetic-app.exe')
        expect(panel.get_by_text(re.compile('slope 69905.066667 bytes/s'))).to_be_visible()
        expect(panel.get_by_text('Private commit grew by 3145728 bytes over 45 s with three increases and no decreases.')).to_be_visible()
        expect(panel.get_by_text(re.compile('exact creation time separates PID reuse'))).to_be_visible()
        expect(panel.locator('svg path')).to_have_count(2)
        expect(panel.get_by_role('img')).to_have_attribute('aria-label', re.compile('fraction over 10 s; 5 samples'))
        expect(panel.get_by_text('Missing samples and sampling gaps remain gaps, not zero.')).to_be_visible()
        panel.get_by_text('Sample facts', exact=True).click()
        expect(panel.locator('.samples')).to_contain_text('unknown')
        expect(panel.locator('.samples')).to_contain_text('0.0%')
        panel.get_by_label('Trend metric').select_option('private_commit_bytes')
        expect(panel.get_by_role('img')).to_have_attribute('aria-label', re.compile('bytes over 45 s; 4 samples'))
        expect(panel.locator('.samples')).to_contain_text('3.00 MiB')
        page.get_by_role('button', name='Pause updates').click()
        expect(panel.get_by_text('Paused; retained observation window.', exact=False)).to_be_visible()
        for theme in ('Observatory', 'Woven atlas'):
            page.get_by_role('group', name='Theme').get_by_role('button', name=theme).click()
            expect(panel.locator('svg path')).to_have_count(1)
        page.set_viewport_size({'width': 320, 'height': 844})
        ok(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), 'resource trends: fits 320 px')
        engine.trend_mode = 'missing'
        page.get_by_role('button', name='Resume updates').click()
        expect(panel.get_by_text('Trend history unavailable or awaiting leased samples.')).to_be_visible()
        expect(panel.get_by_text('Missing samples and denied handles: coverage unknown.')).to_be_visible()
        expect(panel.locator('svg')).to_have_count(0)
        engine.trend_mode = 'refused'
        expect(page.get_by_text('Telemetry unavailable, retrying')).to_be_visible(timeout=10000)
        expect(panel.get_by_text('growth candidate', exact=True)).to_have_count(0)
        ok(not errors, f'resource trends: no page errors {errors}')
        ok(panel.get_by_role('button', name=FORBIDDEN_BUTTONS).count() == 0, 'resource trends: no effect controls')
        print('PASS LW-051: windows, units, fact citations, gaps, pause, themes and missing/refused samples')
    finally:
        page.close()
        engine.stop()


def check_process_strip_failures(browser) -> None:
    engine = FakeEngine()
    engine.process_error = ''
    engine.start()
    page = browser.new_page()
    errors = []
    page.on('pageerror', lambda e: errors.append(str(e)))
    start = datetime(2026, 10, 10, 12, tzinfo=timezone.utc)
    page.clock.install(time=start)
    try:
        page.goto(f'{engine.base}/#token={TOKEN}')
        page.evaluate("location.hash = '#/observatory'")
        strip = page.get_by_role('region', name='Processes', exact=True)
        expect(strip.get_by_role('link')).to_have_count(1)
        engine.invalidate()
        for _ in range(30):
            if len(engine.telemetry_calls) >= 3:
                break
            page.wait_for_timeout(100)
        # The release is issued before the new subscribe, but both are separate requests, so their
        # arrival order at the engine is not guaranteed: check the set, not the order.
        first = engine.telemetry_calls[:3]
        ok(first and first[0]['command'] == 'telemetry.subscribe'
           and sorted(r['command'] for r in first) == ['telemetry.subscribe', 'telemetry.subscribe', 'telemetry.unsubscribe'],
           'process strip: session epoch change releases the old lease and subscribes once again')
        ok([r['payload']['subscription_id'] for r in first if r['command'] == 'telemetry.unsubscribe'] == ['sub_fake_1'],
           'process strip: epoch change releases the correct subscription')
        page.evaluate("""() => {
          window.__stripStatus = document.querySelector('#strip-status');
          window.__stripAnnouncements = [];
          new MutationObserver(() => window.__stripAnnouncements.push({
            text: window.__stripStatus.textContent, focus: document.activeElement.id
          })).observe(window.__stripStatus, {childList: true, characterData: true, subtree: true});
        }""")
        for i, code in enumerate(('partial_coverage', 'capability_unavailable'), 1):
            strip.get_by_role('link').first.focus()
            engine.process_error = code
            page.clock.set_system_time(start + timedelta(seconds=16 * (2 * i - 1)))
            expect(strip.get_by_role('status')).to_contain_text('not sampled' if code == 'partial_coverage' else 'unavailable')
            expect(strip.get_by_text('synthetic process observation failure', exact=True)).to_be_visible()
            expect(strip.get_by_role('link')).to_have_count(0)
            expect(strip.get_by_role('heading', name='Processes', exact=True)).to_be_focused()
            ok(True, f'process strip: {code} stays visible and moves row focus to the stable heading')
            spoken = 'not sampled' if code == 'partial_coverage' else 'unavailable'
            ok(page.evaluate("""text => {
              const status = window.__stripStatus;
              return status === document.querySelector('#strip-status') && status.isConnected
                && status.getAttribute('role') === 'status' && status.getAttribute('aria-live') === 'polite'
                && status.getAttribute('aria-atomic') === 'true'
                && window.__stripAnnouncements.some(a => a.text.includes(text) && a.focus === 'strip-h');
            }""", spoken), f'process strip: {code} updates the persistent live region after the focus move')
            engine.process_error = ''
            page.clock.set_system_time(start + timedelta(seconds=32 * i))
            expect(strip.get_by_role('link')).to_have_count(1)
        engine.process_error = 'partial_coverage'
        engine.explain_partial_once = True
        strip.get_by_role('link').first.click()
        expect(page.get_by_role('heading', name='Companion', level=1)).to_be_visible()
        expect(page.get_by_role('status').filter(has_text='Waiting for a fresh process sample.')).to_be_visible()
        ok(len(engine.explain_calls) == 1 and page.locator('main [role=alert]').filter(has_text='synthetic process sample not ready').count() == 0,
           'Companion deep link: partial coverage waits honestly without a stale explanation alert')
        engine.process_error = ''
        page.clock.set_system_time(start + timedelta(seconds=80))
        expect(page.locator('table.procs tbody tr')).to_have_count(1)
        expect(page.get_by_role('status').filter(has_text='Waiting for a fresh process sample.')).to_be_visible()
        ok(len(engine.explain_calls) == 1, 'Companion deep link: retained observations do not trigger a retry')
        engine.process_observed_at = (start + timedelta(seconds=96)).isoformat().replace('+00:00', 'Z')
        page.clock.set_system_time(start + timedelta(seconds=96))
        expect(page.get_by_role('region', name='Explanation')).to_be_visible(timeout=10000)
        expect(page.get_by_role('heading', name='About this process')).to_be_focused()
        ok(len(engine.explain_calls) == 2 and engine.explain_calls[0] == engine.explain_calls[1],
           'Companion deep link: its first process sample retries the same opaque reference once')
        ok(not errors, f'process strip failures: no page errors {errors}')
    finally:
        page.close()
        engine.stop()


def check_process_strip(browser, base: str, shots: Path | None) -> None:
    page = browser.new_page(viewport={'width': 1440, 'height': 1000})
    errors = []
    page.on('pageerror', lambda e: errors.append(str(e)))
    start = datetime.now(timezone.utc)
    page.clock.install(time=start)
    page.goto(f'{base}/?transport=mock#/observatory')
    strip = page.get_by_role('region', name='Processes', exact=True)
    expect(strip.locator('li')).to_have_count(5)
    expect(strip.get_by_role('status')).to_contain_text('Synthetic process observations')
    expect(strip.get_by_role('status')).to_contain_text('could not be read (unknown, not zero)')
    expect(strip.locator('.process-metric').first).to_contain_text('unknown')
    ok('0.0%' not in strip.inner_text(), 'process strip: initial CPU rates are unknown, never zero')
    retained_at = strip.locator('time').first.get_attribute('datetime')

    page.clock.set_system_time(start + timedelta(seconds=12))
    wait_until(page, '() => document.querySelector(".process-strip time").textContent.includes("12 s ago")')
    ok(strip.locator('time').first.get_attribute('datetime') == retained_at, 'process strip: newer gauge samples do not refresh row observation time')
    page.clock.set_system_time(start + timedelta(seconds=16))
    wait_until(page, '() => document.querySelector(".process-strip .process-metric").textContent.includes("%")')
    expect(strip.locator('time').first).not_to_contain_text('age unknown')
    cpu = [float(t.split('%')[0]) for t in strip.locator('.process-metric').all_inner_texts()]
    ok(cpu == sorted(cpu, reverse=True), f'process strip: descending machine-normalised CPU {cpu}')

    page.get_by_label('Process lens').select_option('private_desc')
    expect(strip.locator('.process-name').first).to_have_text('vm-host.exe')
    expect(strip.locator('.process-metric').first).to_contain_text('6.80 GiB')
    ok(strip.get_by_role('button', name=FORBIDDEN_BUTTONS).count() == 0, 'process strip has no process effect controls')

    # An update cannot reorder a row while its explanation link has focus.
    strip.get_by_role('link').first.focus()
    held_at = strip.locator('time').first.get_attribute('datetime')
    held_order = strip.locator('.process-name').all_inner_texts()
    page.clock.set_system_time(start + timedelta(seconds=32))
    expect(strip.get_by_role('status')).to_contain_text('A newer observation is waiting')
    ok(strip.locator('.process-name').all_inner_texts() == held_order and strip.locator('time').first.get_attribute('datetime') == held_at,
       'process strip: row order and observations hold under keyboard focus')
    page.get_by_label('Process lens').focus()
    expect(strip.get_by_role('status')).not_to_contain_text('A newer observation is waiting')
    ok(strip.locator('time').first.get_attribute('datetime') != held_at, 'process strip: focus leaving releases the queued observation')

    page.get_by_role('button', name='Pause updates').click()
    expect(strip.get_by_role('status')).to_contain_text('Paused; showing retained observations')
    paused_at = strip.locator('time').first.get_attribute('datetime')
    page.clock.set_system_time(start + timedelta(seconds=44))
    wait_until(page, '() => document.querySelector(".process-strip time").textContent.includes("12 s ago")')
    ok(strip.locator('time').first.get_attribute('datetime') == paused_at, 'process strip: paused retained rows still show their own age')
    page.get_by_role('button', name='Resume updates').click()
    expect(page.get_by_label('Process lens')).to_be_enabled()

    if shots:
        for theme, label in (('woven-atlas', 'Woven atlas'), ('observatory', 'Observatory')):
            page.get_by_role('group', name='Theme').get_by_role('button', name=label).click()
            strip.screenshot(path=str(shots / f'process-strip-{theme}.png'))
        page.set_viewport_size({'width': 320, 'height': 844})
        strip.screenshot(path=str(shots / 'process-strip-320.png'))
    page.set_viewport_size({'width': 320, 'height': 844})
    ok(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), 'process strip: no horizontal page scroll at 320 px')
    ok(strip.locator('time').first.is_visible(), 'process strip: row ages stay visible at 320 px')

    strip.get_by_role('link').first.focus()
    page.keyboard.press('Enter')
    expect(page.get_by_role('heading', name='Companion', level=1)).to_be_visible()
    explanation = page.get_by_role('region', name='Explanation')
    expect(explanation).to_contain_text('vm-host.exe')
    expect(explanation.get_by_role('heading')).to_be_focused()
    ok(explanation.get_by_role('button').count() == 0, 'process strip: Enter opens the row explanation in Companion, read-only')
    page.goto(f'{base}/?transport=mock#/companion?process=pc_missing')
    expect(page.locator('main [role=alert]').filter(has_text='no such process')).to_be_visible()
    ok(page.get_by_role('region', name='Explanation').count() == 0, 'an expired or missing process reference is not explained as a different process')
    ok(not errors, f'process strip: no page errors {errors}')
    page.close()


def check_companion(page: Page, base: str, shoot) -> None:
    page.goto(f'{base}/?transport=mock#/companion')
    expect(page.get_by_role('heading', name='Companion', level=1)).to_be_visible()
    main = page.locator('main')
    expect(main.get_by_text('Observation only')).to_be_visible()
    expect(rows(page).first).to_be_visible()
    observed = page.locator('table.procs tbody tr').first.locator('time[datetime]')
    expect(observed).to_be_visible()
    expect(observed).to_have_text(observed.get_attribute('datetime'))
    ok(bool(re.fullmatch(r'\d{4}-\d{2}-\d{2}T.*Z', observed.get_attribute('datetime') or '')), 'each process row exposes its own observation time')
    ok(main.get_by_role('button', name=FORBIDDEN_BUTTONS).count() == 0, 'no control to end, pause, re-prioritise or trim anything')
    # shared form patterns must not restyle this view's own toolbar (Codex on #345)
    ok(main.locator('.ctl').first.evaluate('e => getComputedStyle(e).flexDirection') == 'row', 'the Companion toolbar stays a wrapping row')

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
    wait_until(page, '(b) => { const m = /Sample ([\\d,]+) at/.exec(document.querySelector("main").innerText); return m && Number(m[1].replace(/,/g, "")) > b; }', before, 9000)
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
    ok(main.get_by_text('off', exact=True).count() == 10, 'the capability matrix shows all 10 file/process effects off')
    reads = main.locator('.caps li').filter(has_text='Content read: duplicates only, on request')
    expect(reads).to_contain_text('not available')
    ok(main.locator('.caps li').filter(has_text=re.compile(r'\boff\s+content read\b', re.I)).count() == 0,
       'content reads are separate from file/process effects')
    expect(main.get_by_text('The one exception is a diagnostic bundle you save yourself')).to_be_visible()
    ok(main.get_by_text('ON', exact=True).count() == 0, 'no effect is on')
    expect(main.get_by_text('not available').first).to_be_visible()
    diag = main.locator('section[aria-labelledby="h-diag"]')
    expect(diag.get_by_text('the text below is exactly these bytes')).to_be_visible()
    expect(diag.get_by_role('region', name='Diagnostic bundle preview')).to_contain_text('"loomward-diagnostics/1"')
    ok(diag.get_by_role('button', name='Save…').count() == 0, 'the mock (like HTTP) offers no save')
    expect(diag.get_by_text('Saving needs the desktop app')).to_be_visible()
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

    # forget the revoked root (owner decision q-9): asked with the folder named, confirmed, then only a dated record remains
    grants = main.locator('section[aria-labelledby="h-grants"]')
    shown = first_root.locator('th').inner_text().splitlines()[0]
    forget = first_root.get_by_role('button', name=re.compile('^Forget…'))
    expect(forget).to_have_count(1)
    forget.click()
    ask = page.get_by_role('group', name=re.compile('^Forget '))
    expect(ask).to_be_focused()
    expect(ask).to_contain_text('removes its name and catalogue from its database and its own backup copies, and discards the start-up backup copies')
    expect(ask).to_contain_text('traces may remain in unused disk space until it is overwritten. Your files are not touched.')
    ask.get_by_role('button', name='Keep it').click()
    expect(ask).to_have_count(0)
    expect(forget).to_be_focused()  # a cancelled question hands focus back to the button that asked it
    forget.click()
    ask.get_by_role('button', name='Forget', exact=True).click()
    expect(page.get_by_role('status').filter(has_text='Forgotten')).to_contain_text('No scanned file was touched')
    ok(shown not in roots_table.inner_text(), 'the forgotten root is gone from the roots table')
    record = grants.locator('li', has_text='name forgotten')
    expect(record).to_have_count(1)
    expect(record).to_contain_text('revoked')
    expect(record).to_contain_text('· forgotten')
    ok(shown not in grants.inner_text(), 'the grants list keeps the record without the name')
    expect(record.get_by_role('button')).to_have_count(0)

    # revoke the teacher disclosure grant
    grants.locator('li', has_text='Teacher disclosure').get_by_role('button', name='Revoke…').click()
    page.get_by_role('group', name=re.compile('Revoke grant')).get_by_role('button', name='Revoke', exact=True).click()
    expect(grants.locator('li', has_text='Teacher disclosure')).to_contain_text('revoked')
    expect(grants.locator('li', has_text='Teacher disclosure').get_by_role('button')).to_have_count(0)
    print('PASS Grants & health: revoke root and grant with confirmation, forget a revoked root, state reflected, all effects off')


ROOTS = 'main section[aria-labelledby="h-roots"]'
SCAN_CLEAN, SCAN_DIRTY = '[mock] lab-complete', '[mock] lab-unreadable-folders'


def check_scan_controls(page: Page, base: str, shots: Path | None) -> None:
    """Scan, Refresh and Cancel on a granted root, on the mock transport: a lab root scans, a fixture root is
    refused with the service's reason, a running scan is indeterminate, a partial scan says partial."""
    page.set_viewport_size({'width': 1440, 'height': 900})
    page.goto(f'{base}/?transport=mock#/health')
    page.reload()  # a same-URL goto is only a hash change: reload for a fresh mock
    expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
    section = page.locator(ROOTS)
    live = section.locator('p.scanlive')
    cards = section.locator('ul.scans li')
    clean = cards.filter(has_text=SCAN_CLEAN)
    dirty = cards.filter(has_text=SCAN_DIRTY)
    fixture = cards.first
    table_row = lambda root: section.locator('tbody tr', has_text=root)

    def shoot(name: str) -> None:
        if not shots:
            return
        for tag, (w, h) in VIEWPORTS.items():
            page.set_viewport_size({'width': w, 'height': h})
            page.wait_for_timeout(300)
            head, body = section.locator('#h-scans').bounding_box(), section.locator('ul.scans').bounding_box()
            top = page.evaluate('scrollY')
            page.screenshot(path=str(shots / f'{name}-{tag}.png'), full_page=True, clip={
                'x': max(0, head['x'] - 8), 'y': head['y'] + top - 8, 'width': body['width'] + 16, 'height': body['y'] + body['height'] - head['y'] + 16})
        page.set_viewport_size({'width': 1440, 'height': 900})

    def scan_btn(row, root: str):
        return row.get_by_role('button', name=f'Scan {root}', exact=True)

    def cancel_btn(row, root: str):
        return row.get_by_role('button', name=f'Cancel scan of {root}', exact=True)

    # idle: a Scan button on each active root, no Refresh before a first scan, a status region that is already in the page
    expect(live).to_have_attribute('role', 'status')
    ok(live.inner_text() == '', 'the scan status region is in the page and empty before any scan')
    expect(scan_btn(clean, SCAN_CLEAN)).to_be_visible()
    expect(clean.get_by_role('button', name=re.compile('^Refresh'))).to_have_count(0)
    expect(table_row(SCAN_CLEAN)).to_contain_text('never scanned')
    ok(not re.search(r'Scanning|Finished|Cancelled', clean.inner_text()), 'an unscanned root claims no scan result')
    ok(clean.get_by_role('button').count() == 1, 'an unscanned root offers Scan and nothing else')
    shoot('scan-idle')

    # refused: the mock answers a fixture root the way the service answers a non-lab root in a synthetic session
    fixture.get_by_role('button', name=re.compile('^Scan ')).click()
    expect(fixture.locator('.problem')).to_contain_text('registered lab roots only')
    expect(live).to_contain_text('registered lab roots only')
    ok(fixture.locator('.job').count() == 0, 'a refused scan leaves no scan job on the root')
    shoot('scan-refused')

    # running: indeterminate (text and elapsed time, no bar that fills, no percentage), Cancel in the same place
    scan_btn(clean, SCAN_CLEAN).click()
    expect(cancel_btn(clean, SCAN_CLEAN)).to_be_focused()
    expect(clean).to_contain_text('Scanning')
    expect(clean).to_contain_text('No progress is reported until the scan ends')
    expect(clean).to_contain_text(re.compile(r'elapsed [1-9]\d* s'))
    ok(clean.locator('[role=progressbar], [role=meter], progress, meter').count() == 0, 'a running scan has no progress bar or meter')
    ok('%' not in clean.inner_text(), 'a running scan shows no percentage')
    expect(live).to_contain_text('Scanning')
    shoot('scan-running')

    # completed: complete coverage only because the mock root is complete; counts; focus stays on the one button
    expect(clean).to_contain_text('Finished, complete coverage', timeout=9000)
    expect(clean).to_contain_text('1,284 files, 212 folders')
    expect(clean).to_contain_text(re.compile(r'took \d+ s'))
    expect(live).to_contain_text('Finished, complete coverage')
    expect(scan_btn(clean, SCAN_CLEAN)).to_be_focused()
    expect(clean.get_by_role('button', name=f'Refresh scan of {SCAN_CLEAN}', exact=True)).to_be_visible()
    ok('partial' not in clean.locator('.job').inner_text().lower(), 'a complete scan is not called partial')
    expect(table_row(SCAN_CLEAN).locator('td').nth(4)).to_have_text('1,284')

    # refresh then cancel: Refresh goes away, focus follows to Cancel, the root ends cancelled and not complete-covered
    clean.get_by_role('button', name=f'Refresh scan of {SCAN_CLEAN}', exact=True).click()
    expect(cancel_btn(clean, SCAN_CLEAN)).to_be_focused()
    cancel_btn(clean, SCAN_CLEAN).click()
    expect(clean).to_contain_text('Cancelled', timeout=5000)
    expect(clean).to_contain_text('not published as a complete scan')
    expect(live).to_contain_text('Cancelled')

    # partial: the root has unreadable folders, so the finished scan says partial and keeps the gap in view
    scan_btn(dirty, SCAN_DIRTY).click()
    expect(dirty).to_contain_text('Finished, partial coverage', timeout=9000)
    expect(dirty).to_contain_text('4 could not be read')
    expect(dirty).to_contain_text('lower bounds')
    ok('complete coverage' not in dirty.locator('.job').inner_text().lower(), 'a partial scan is never called complete')
    expect(table_row(SCAN_DIRTY).locator('td').nth(3)).to_contain_text('partial')
    shoot('scan-partial')

    # leaving the view mid-scan loses nothing: the Explorer refetches on tree.invalidated, and the result is here on return
    scan_btn(dirty, SCAN_DIRTY).click()
    expect(cancel_btn(dirty, SCAN_DIRTY)).to_be_visible()
    page.evaluate("location.hash = '#/explorer'")
    expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
    page.wait_for_timeout(5200)
    expect(rows(page).first).to_be_visible()
    page.evaluate("location.hash = '#/health'")
    expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
    expect(section.locator('ul.scans li', has_text=SCAN_DIRTY)).to_contain_text('Finished, partial coverage')
    ok(section.locator('p.scanlive').inner_text() == '', 'a scan that ended while the view was away is not announced again on return')
    print('PASS scan controls: idle, refused, running indeterminate, complete, refresh and cancel, partial, survives leaving the view')


def check_add_folder(page: Page, engine: FakeEngine) -> None:
    """'Add a folder…' against an engine with a native picker: waiting, granted, cancelled, refused, busy and
    the deadline paths. The page never sends a path, and a deadline never reopens the dialog."""
    engine.grant_picker = True
    page.set_viewport_size({'width': 1360, 'height': 900})

    def fresh() -> None:
        engine.roots, engine.grant_calls = [], []
        page.goto(f'{engine.base}/#token={TOKEN}')
        page.reload()
        page.evaluate("location.hash = '#/health'")
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
        expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()

        reads = page.locator('.caps li').filter(has_text='Content read: duplicates only, on request')
        expect(reads).to_contain_text('available')
        expect(reads).not_to_contain_text('not available')

    add = page.get_by_role('button', name='Add a folder…', exact=True)
    status = page.locator('main section[aria-labelledby="h-roots"] .add p.live')
    cards = page.locator('main ul.scans li')
    table = page.locator('main table.tbl tbody')
    error = page.locator('main p.bad.live')

    # granted: waiting text while the dialog is open (the button stays, aria-disabled, so focus is not lost), then the root and its Scan button
    engine.grant_mode, engine.grant_delay = 'granted', 1.0
    fresh()
    expect(table).to_contain_text('No roots granted')
    ok(status.get_attribute('role') == 'status' and status.inner_text() == '', 'the add-folder status region is in the page and empty before any click')
    add.click()
    expect(status).to_have_text('Waiting for you to choose a folder in the Windows dialog.')
    expect(add).to_have_attribute('aria-disabled', 'true')
    expect(add).to_be_focused()
    add.click(force=True)  # a second click while the dialog is open sends nothing (force: Playwright waits out aria-disabled otherwise)
    page.evaluate('document.activeElement.blur()')  # the native dialog took the window focus: it must come back to the button
    expect(status).to_contain_text('Added E:\\Picked Folder', timeout=5000)
    ok(len(engine.grant_calls) == 1, 'one dialog, one request, however often the button is pressed while waiting')
    call = engine.grant_calls[0]
    ok(call['payload'] == {'purpose': 'metadata_scan'} and call['deadline_ms'] == 60000, f"the request carries no path and asks for the longest deadline: {call['payload']} {call['deadline_ms']}")
    expect(table).to_contain_text('Picked Folder')
    expect(add).not_to_have_attribute('aria-disabled', 'true')
    expect(add).to_be_focused()
    card = cards.filter(has_text='Picked Folder')
    expect(card.get_by_role('button', name=re.compile('^Scan ')).first).to_be_visible()
    expect(card).to_contain_text('just added')
    ok(error.inner_text() == '', 'a granted folder shows no error')
    page.evaluate("location.hash = '#/explorer'")
    expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
    page.evaluate("location.hash = '#/health'")
    expect(page.get_by_role('heading', name='Grants & health', level=1)).to_be_visible()
    expect(table).to_contain_text('Picked Folder')
    ok(status.inner_text() == '' and cards.filter(has_text='just added').count() == 0, 'leaving Health and coming back does not repeat "Added" or the just-added tag')

    # cancelled: quiet and neutral, no root
    engine.grant_mode, engine.grant_delay = 'cancelled', 0.0
    fresh()
    add.click()
    expect(status).to_have_text('No folder was added.')
    expect(add).to_be_focused()
    ok('Picked' not in table.inner_text() and error.inner_text() == '', 'cancelling adds nothing and raises no error')

    # refused (the unverified-identity refusal): the reason as plain text
    engine.grant_mode = 'refused'
    fresh()
    add.click()
    expect(status).to_contain_text('could not verify that folder')
    expect(table).to_contain_text('No roots granted')

    # busy: a dialog is still open elsewhere, which is not a failure
    engine.grant_mode = 'busy'
    fresh()
    add.click()
    expect(status).to_have_text('A Windows folder dialog is still open. Choose a folder there and it will appear below.')
    expect(add).to_be_focused()

    # deadline, the grant landed: the roots are read again and the new one is reported; the dialog is not reopened
    engine.grant_mode = 'deadline'
    fresh()
    add.click()
    expect(status).to_contain_text('Added E:\\Picked At The Deadline')
    ok(len(engine.grant_calls) == 1, 'deadline_exceeded never reopens the picker')

    # deadline, nothing yet, the owner picks afterwards: the status says so, and the root appears on roots.changed
    engine.grant_mode = 'deadline_late'
    fresh()
    add.click()
    expect(status).to_contain_text('No folder was added yet')
    expect(table).to_contain_text('Picked After The Deadline', timeout=5000)
    expect(status).to_contain_text('Added E:\\Picked After The Deadline')  # the late grant is worded as added, not left at the deadline text
    ok(len(engine.grant_calls) == 1, 'a grant that lands after the deadline shows up from roots.changed without a second request')

    # no picker: the button is gone and the page says where folders are added
    engine.grant_picker = False
    fresh()
    expect(page.get_by_role('button', name='Add a folder…')).to_have_count(0)
    expect(page.locator('main')).to_contain_text('Folders are added in the desktop app.')
    print('PASS Add a folder: waiting, granted, cancelled, refused, busy, deadline then re-read, no picker')


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
    check_process_strip(page.context.browser, base, shots)
    check_process_strip_failures(page.context.browser)
    check_resource_trends(page.context.browser)
    check_explorer_mock(page, base, shoot)
    engine = FakeEngine()
    engine.start()
    try:
        check_explorer_engine(page, engine)
        check_explorer_starts(page, engine)
        check_explorer_fallback(page, engine)
        check_explorer_reissued_ids(page, engine)
        check_tiers_loading_followups(page, engine)
        check_tiers_source_eligibility(page, engine)
        check_tiers_assumption_refresh(page, engine)
        check_tiers_refused(page, engine)
        check_backups_import(page, engine)
        check_diagnostics_save(page, engine)
        check_add_folder(page, engine)
    finally:
        engine.stop()
    check_tiers(page, base, shoot)
    check_companion(page, base, shoot)
    check_health(page, base, shoot)
    check_backups(page, base, shoot)
    check_alerts(page, base, shoot)
    check_scan_controls(page, base, shots)
    check_narrow(page, base)
    check_narrow_wide_fonts(page, base)
