"""Explorer scope over controlled HTTP replies and SSE, with real keyboard input."""
from copy import deepcopy
import json
from pathlib import Path

from playwright.sync_api import expect


def run(ctx):
    page = ctx.page.context.new_page()
    root = Path(__file__).resolve().parents[3]
    examples = root / 'contracts/v3/examples/commands'
    load = lambda name: json.loads((examples / f'{name}.result.json').read_text(encoding='utf-8'))
    roots = []
    for i in range(64):
        row = deepcopy(load('roots.list')['roots'][0])
        row.update(root_id=f'rt_scope_{i}', display_path={'text': f'Synthetic root {i}', 'truncated': False})
        roots.append(row)
    hostile = 'Synthetic A <img src=x onerror=alert(1)>\u202e'
    roots[0]['display_path'] = {'text': hostile, 'truncated': True}
    roots[1]['display_path']['text'] = 'Synthetic B'
    roots[2]['grant_state'] = 'revoked'
    calls = []
    partial = False
    template = load('search.query')
    hits = []
    for r in roots[:2]:
        for i in range(51):
            hit = deepcopy(template['hits'][0])
            hit.update(root_id=r['root_id'], highlights=[])
            hit['row'].update(node_id=f'nd_{r["root_id"]}_{i}', name=f'shared-{i:03}.pdf', extension='pdf',
                              location_hint={'text': r['display_path']['text'], 'truncated': True})
            hits.append(hit)

    def reply(route):
        req = route.request.post_data_json
        command, payload = req['command'], req['payload']
        if command == 'roots.list':
            result = {'roots': roots}
        elif command == 'tree.slice':
            result = {**load('tree.slice'), 'nodes': [], 'edges': []}
        elif command == 'search.query':
            calls.append(payload)
            selected = [h for h in hits if payload['root_id'] is None or h['root_id'] == payload['root_id']]
            if payload['text'] == 'missing':
                selected = []
            offset = int(payload['cursor'] or '0')
            result = {**template, 'hits': selected[offset:offset + 50], 'partial': partial,
                      'next_cursor': str(offset + 50) if len(selected) > offset + 50 else None,
                      'roots': [{'root_id': r['root_id'], 'scan_state': 'complete', 'coverage': 'complete'}
                                for r in roots if r['grant_state'] == 'active' and
                                (payload['root_id'] is None or payload['root_id'] == r['root_id'])]}
        elif command == 'session.hello':
            result = load('session.hello')
            result['dataset_class'] = 'synthetic'
        elif command == 'volumes.list':
            result = load('volumes.list')
        else:
            route.continue_()
            return
        route.fulfill(json={'protocol': 'loomward/3', 'request_id': req['request_id'], 'ok': True, 'result': result,
                            'meta': {'served_at': '2026-10-01T00:00:00Z', 'elapsed_ms': 0, 'dataset_class': 'synthetic',
                                     'budget_hit': False, 'catalog_rev': '1', 'state_rev': '1'}})

    # Keep the real HTTP/SSE parser and session lifecycle; control only the fixture stream.
    page.add_init_script("""(() => {
      const realFetch = window.fetch.bind(window);
      let seq = 0;
      window.scopeEmit = (event, data) => {
        const frame = {protocol:'loomward/3', epoch:'e_search_scope', seq:seq++, event,
          at:'2026-10-01T00:00:00Z', catalog_rev:'1', state_rev:'1', data};
        window.scopeStream.enqueue(new TextEncoder().encode(
          `id: ${frame.epoch}.${frame.seq}\\nevent: ${event}\\ndata: ${JSON.stringify(frame)}\\n\\n`));
      };
      window.fetch = (input, init) => {
        const url = new URL(typeof input === 'string' ? input : input.url, location.href);
        if (url.pathname !== '/api/v3/events') return realFetch(input, init);
        if (window.scopeOffline) return Promise.reject(new Error('Synthetic disconnect'));
        return Promise.resolve(new Response(new ReadableStream({start(controller) {
          window.scopeStream = controller;
          window.scopeEmit('stream.hello', {session_started_at:'2026-10-01T00:00:00Z', epoch:'e_search_scope',
            last_seq:seq, oldest_replayable_seq:0, dataset_class:'synthetic'});
          init.signal.addEventListener('abort', () => { try { controller.close(); } catch {} }, {once:true});
        }}), {headers:{'Content-Type':'text/event-stream'}}));
      };
    })();""")
    page.route('**/api/v3/call', reply)
    try:
        page.goto(f'{ctx.base}/#token=a1b2c3d4e5f60718293a4b5c6d7e8f90')
        page.evaluate("location.hash = '#/explorer?q=shared'")
        scope = page.get_by_label('Search scope', exact=True)
        applied = page.get_by_label('Applied search scope', exact=True)
        box = page.get_by_placeholder('Search names')
        rows = page.locator('main table tbody tr')
        expect(scope.locator('option')).to_have_count(64)  # All + 63 active; not the 16-node Atlas slice.
        expect(scope).to_have_value('')
        expect(applied).to_have_text('Scope: All granted roots')
        expect(rows).to_have_count(50)
        expect(scope.locator('option[value="rt_scope_63"]')).to_have_count(1)
        expect(scope.locator('option[value="rt_scope_2"]')).to_have_count(0)
        expect(scope.locator('option[value="rt_scope_0"]')).to_contain_text('U+202E RLO (truncated)')
        ctx.check(calls[-1]['root_id'] is None, 'An unscoped q link searches all granted roots')

        for width in [1440, 390]:
            page.set_viewport_size({'width': width, 'height': 900})
            scope.focus()
            scope.press('Home')
            scope.press('ArrowDown')
            scope.press('Tab')
            expect(scope).to_have_value('rt_scope_0')
            expect(applied).to_have_text('Scope: All granted roots')
            box.focus()
            box.press('Enter')
            expect(applied).to_contain_text('Synthetic A <img src=x onerror=alert(1)>')
            expect(applied).to_contain_text('U+202E RLO')
            expect(applied).to_contain_text('truncated')
            expect(rows).to_have_count(50)
            box.press('ArrowDown')
            expect(page.locator('button[data-inspect]').first).to_be_focused()
            page.keyboard.press('Escape')
            expect(box).to_be_focused()
            scope.focus()
            scope.press('ArrowDown')
            scope.press('Tab')
            expect(scope).to_have_value('rt_scope_1')
            expect(applied).to_contain_text('Synthetic A')
            more = page.get_by_role('button', name='Load more', exact=True)
            more.focus()
            more.press('Enter')
            expect(rows).to_have_count(51)
            ctx.check(calls[-1]['root_id'] == 'rt_scope_0' and calls[-1]['cursor'] == '50',
                      f'Submitted A scope survives a draft B choice and keyboard paging at {width}px')
            expect(page.locator('main img, main script, main iframe')).to_have_count(0)
            ctx.check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'Search scope fits at {width}px')
            captures = root / '.loomward'
            captures.mkdir(exist_ok=True)
            page.screenshot(path=str(captures / f'search-root-{width}.png'), full_page=True)
            scope.select_option('')
            box.press('Enter')
            expect(applied).to_have_text('Scope: All granted roots')
            expect(rows).to_have_count(50)

        scope.select_option('rt_scope_0')
        box.press('Enter')
        expect(applied).to_contain_text('Synthetic A')
        roots[0]['grant_state'] = 'revoked'
        page.evaluate("roots => window.scopeEmit('roots.changed', {roots})", roots)
        expect(applied).to_contain_text('Search paused')
        expect(rows).to_contain_text('No results are shown until you choose an available scope')
        expect(scope).to_have_value('')  # null placeholder has no serialised root ID.
        expect(scope.locator('option:checked')).to_have_text('Choose a search scope')
        before = len(calls)
        box.press('Enter')
        expect(page.locator('#explorer-notice')).to_contain_text('Choose an available search scope')
        ctx.check(len(calls) == before, 'Revocation never silently widens a search')
        scope.select_option('rt_scope_1')
        box.press('Enter')
        expect(applied).to_have_text('Scope: Synthetic B')
        page.evaluate('window.scopeOffline = true; window.scopeStream.close()')
        expect(page.get_by_role('heading', name='Unavailable', exact=True)).to_be_visible(timeout=15000)
        before = len(calls)
        page.evaluate('window.scopeOffline = false')
        expect(page.get_by_role('heading', name='Explorer', exact=True)).to_be_visible(timeout=20000)
        expect(applied).to_contain_text('Search paused')
        ctx.check(len(calls) == before, 'Reconnect does not replay the old q link across all roots')
        scope.select_option('rt_scope_1')
        box.press('Enter')
        expect(applied).to_have_text('Scope: Synthetic B')
        partial = True
        box.fill('missing')
        box.press('Enter')
        expect(rows).to_contain_text('coverage is partial')
        expect(page.get_by_role('list', name='Search coverage')).to_contain_text('not proof of absence')
        page.get_by_role('button', name='Clear search', exact=True).click()
        expect(box).to_be_focused()
        page.get_by_role('group', name='Cleanup searches').get_by_role('button', name='Disk images over 1 GB').click()
        expect(applied).to_have_text('Scope: Synthetic B')
        ctx.check(calls[-1]['root_id'] == 'rt_scope_1' and calls[-1]['extension'] == 'iso', 'Presets retain the chosen scope')
        page.evaluate("location.hash = '#/explorer?q=another'")
        expect(applied).to_have_text('Scope: All granted roots')
        ctx.check(calls[-1]['root_id'] is None, 'An explicit new unscoped link restores the global default')
        return {'scope': 'pass', 'keyboard_widths': [1440, 390], 'roots': 64, 'reconnect': 'pass'}
    finally:
        page.close()
