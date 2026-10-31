"""Scoped search and selected-row membership over controlled HTTP/SSE, with real keyboard input."""
from copy import deepcopy
import json
from pathlib import Path

from playwright.sync_api import expect


def run(ctx):
    page = ctx.page.context.new_page()
    page.on('pageerror', lambda error: print(f'Search scope page error: {error}'))
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

    updates, commands = [], []
    state_rev = 1
    collection_failure = None
    human_collections = True
    members = {'co_papers': set(), 'co_project': set()}
    collection_rows = [
        {'collection_id': key, 'name': name, 'kind': kind, 'member_count': 0,
         'logical_bytes': '0', 'created_at': '2026-10-01T00:00:00Z'}
        for key, name, kind in [('co_papers', 'Papers', 'human'),
                                ('co_project', 'Project <img src=x>\u202e', 'human'),
                                ('co_label', 'Automatic label view', 'label_view')]
    ]

    def reply(route):
        nonlocal state_rev
        req = route.request.post_data_json
        command, payload = req['command'], req['payload']
        commands.append(command)
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
        elif command == 'collections.list':
            result = {'collections': [dict(c, member_count=len(members.get(c['collection_id'], [])))
                                      for c in collection_rows if human_collections or c['kind'] != 'human']}
        elif command == 'collections.members':
            ids = sorted(members[payload['collection_id']])
            result = {'collection_id': payload['collection_id'], 'items': [
                {'member_id': f'om_{i + 1}', 'status': 'resolved',
                 'entry': next(h['row'] for h in hits if h['row']['node_id'] == node)}
                for i, node in enumerate(ids[:payload['limit']])], 'total': len(ids),
                'next_cursor': None, 'budget_hit': False}
        elif command == 'collections.update_members':
            updates.append(req)
            if collection_failure:
                code, message = collection_failure
                route.fulfill(json={'protocol': 'loomward/3', 'request_id': req['request_id'], 'ok': False,
                                    'error': {'code': code, 'message': message, 'retryable': False, 'detail': None}})
                return
            ctx.check(req.get('expected_state_rev') == str(state_rev), 'Selection uses the freshly read state revision')
            ctx.check(req.get('expected_generation') is None, 'Selected-set membership does not invent a root generation')
            before = members[payload['collection_id']].copy()
            members[payload['collection_id']].update(payload['add'])
            if before != members[payload['collection_id']]:
                state_rev += 1
            result = dict(next(c for c in collection_rows if c['collection_id'] == payload['collection_id']),
                          member_count=len(members[payload['collection_id']]))
        else:
            route.continue_()
            return
        route.fulfill(json={'protocol': 'loomward/3', 'request_id': req['request_id'], 'ok': True, 'result': result,
                            'meta': {'served_at': '2026-10-01T00:00:00Z', 'elapsed_ms': 0, 'dataset_class': 'synthetic',
                                     'budget_hit': False, 'catalog_rev': '1', 'state_rev': str(state_rev)}})

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
        if (url.pathname !== '/api/v3/events') return realFetch(input, init).then(async response => {
          const command = init?.body ? JSON.parse(init.body).command : null;
          const kind = command === 'collections.members' ? 'Read' : command === 'collections.update_members' ? 'Write' : null;
          if (kind && window[`collectionHold${kind}`]) {
            window[`collectionHold${kind}`] = false;
            window[`collection${kind}Waiting`] = true;
            await new Promise(resolve => { window[`collectionRelease${kind}`] = resolve; });
            window[`collection${kind}Waiting`] = false;
          }
          return response;
        });
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

            # Select on the first loaded page, then on its appended page; only explicit IDs are saved.
            first = rows.first.get_by_role('checkbox')
            first.focus()
            first.press('Space')
            last = rows.last.get_by_role('checkbox')
            last.focus()
            last.press('Space')
            target = page.get_by_role('combobox', name='Collection for selected rows', exact=True)
            expect(target.locator('option')).to_have_count(2)
            expect(target).to_be_enabled()
            target.focus()
            target.press('End')
            target.press('Tab')
            expect(target).to_have_value('co_project')
            add = page.get_by_role('button', name='Add selected to collection', exact=True)
            expect(add).to_be_focused()
            before = len(updates)
            add.press('Enter')
            membership = page.get_by_role('status', name='Membership result')
            expect(membership).to_contain_text('Saved selection of 2 rows')
            expect(membership).to_contain_text('U+202E RLO')
            expect(add).to_be_focused()
            expected_ids = ['nd_rt_scope_0_0', 'nd_rt_scope_0_50']
            ctx.check(len(updates) == before + 1 and updates[-1]['payload']['add'] == expected_ids,
                      f'One atomic membership request carries exactly two explicit paged selections at {width}px')
            ctx.check(members['co_project'] == set(expected_ids), 'The controlled membership store contains the selected set')
            revision = state_rev
            add.press('Enter')
            expect(add).to_have_attribute('aria-disabled', 'false')
            expect(membership).to_contain_text('Saved selection of 2 rows')
            ctx.check(state_rev == revision and len(members['co_project']) == 2, 'Re-adding membership is an honest no-op')
            expect(page.get_by_role('button', name='Copy to…', exact=True)).to_be_enabled()
            expect(page.locator('main img, main script, main iframe')).to_have_count(0)
            ctx.check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'Selected membership fits at {width}px')
            page.screenshot(path=str(captures / f'search-root-collection-{width}.png'), full_page=True)
            page.get_by_role('button', name='Clear selection', exact=True).click()
            expect(page.get_by_role('button', name='Add selected to collection', exact=True)).to_have_count(0)
            scope.select_option('')
            box.press('Enter')
            expect(applied).to_have_text('Scope: All granted roots')
            expect(rows).to_have_count(50)

        # A selection can span roots with identical visible filenames; identity is still the node ID.
        rows.first.get_by_role('checkbox').check()
        scope.select_option('rt_scope_1')
        box.press('Enter')
        expect(applied).to_have_text('Scope: Synthetic B')
        expect(rows).to_have_count(50)
        rows.first.get_by_role('checkbox').check()
        target = page.get_by_role('combobox', name='Collection for selected rows', exact=True)
        expect(target).to_have_value('co_papers')
        add = page.get_by_role('button', name='Add selected to collection', exact=True)
        add.click()
        membership = page.get_by_role('status', name='Membership result')
        expect(membership).to_contain_text('Saved selection of 2 rows')
        ctx.check(updates[-1]['payload']['add'] == ['nd_rt_scope_0_0', 'nd_rt_scope_1_0'],
                  'Identical names across scoped searches retain their distinct selected identities')

        # Hold a completed read at the fetch boundary, then change selection before it can issue a write.
        before = len(updates)
        page.evaluate('window.collectionHoldRead = true')
        add.click()
        page.wait_for_function('window.collectionReadWaiting === true')
        rows.nth(1).get_by_role('checkbox').check()
        expect(add).to_have_attribute('aria-disabled', 'false')
        page.evaluate('async () => { window.collectionReleaseRead(); await new Promise(requestAnimationFrame); await new Promise(requestAnimationFrame); }')
        ctx.check(len(updates) == before, 'A superseded revision read cannot submit the old selected set')

        # The service has committed before this held reply: changing selection cannot claim rollback.
        frozen = ['nd_rt_scope_0_0', 'nd_rt_scope_1_0', 'nd_rt_scope_1_1']
        page.evaluate('window.collectionHoldWrite = true')
        add.click()
        page.wait_for_function('window.collectionWriteWaiting === true')
        rows.nth(2).get_by_role('checkbox').check()
        expect(membership).to_contain_text('may already have been saved')
        page.evaluate('async () => { window.collectionReleaseWrite(); await new Promise(requestAnimationFrame); await new Promise(requestAnimationFrame); }')
        expect(membership).to_contain_text('may already have been saved')
        ctx.check(updates[-1]['payload']['add'] == frozen and members['co_papers'] == set(frozen),
                  'The committed frozen set survives a late response without being relabelled as the newer selection')

        for collection_failure in [('stale_generation', 'Collection state changed; refresh before retrying.'),
                                   ('not_found', 'A selected identity was replaced or its root was revoked.'),
                                   ('capability_unavailable', 'A selected row has no durable identity.')]:
            before = len(updates)
            stored = members['co_papers'].copy()
            add.click()
            expect(page.get_by_role('alert').filter(has_text=collection_failure[1])).to_be_visible()
            ctx.check(len(updates) == before + 1 and members['co_papers'] == stored,
                      f'{collection_failure[0]} is shown without retry or partial membership')
            expect(membership).to_be_empty()
        collection_failure = None

        page.get_by_role('button', name='Clear selection', exact=True).click()
        human_collections = False
        rows.first.get_by_role('checkbox').check()
        expect(page.get_by_text('Create a collection in the', exact=False)).to_be_visible()
        expect(page.get_by_role('button', name='Add selected to collection', exact=True)).to_be_disabled()
        page.get_by_role('button', name='Clear selection', exact=True).click()
        human_collections = True
        rows.first.get_by_role('checkbox').check()
        before_disconnect = len(updates)

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
        expect(page.get_by_role('button', name='Add selected to collection', exact=True)).to_have_count(0)
        ctx.check(len(updates) == before_disconnect, 'Reconnect clears session-local selection without replaying membership')
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
        expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
        ctx.check(calls[-1]['root_id'] == 'rt_scope_1' and calls[-1]['extension'] == 'iso', 'Presets retain the chosen scope')
        page.evaluate("location.hash = '#/explorer?q=another'")
        expect(applied).to_have_text('Scope: All granted roots')
        expect(page.locator('main .tbl-wrap')).to_have_attribute('aria-busy', 'false')
        expect(rows).to_have_count(50)
        ctx.check(calls[-1]['root_id'] is None and calls[-1]['text'] == 'another', 'An explicit new unscoped link restores the global default')
        ctx.check(not any(c.startswith('copy.') for c in commands), 'Membership never invokes a copy effect')
        return {'scope': 'pass', 'keyboard_widths': [1440, 390], 'roots': 64, 'reconnect': 'pass',
                'selected_collection': 'pass', 'atomic_refusals': 3, 'late_read_and_write': 'pass'}
    except Exception:
        print(f'Search scope failure URL: {page.url}')
        print(page.locator('body').inner_text()[:6000])
        captures = root / '.loomward'
        captures.mkdir(exist_ok=True)
        page.screenshot(path=str(captures / 'search-root-failure.png'), full_page=True)
        raise
    finally:
        page.close()
