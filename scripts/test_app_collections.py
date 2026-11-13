"""Synthetic collection UI and real loopback service checks; no native root opens."""
from __future__ import annotations
import sqlite3
from contextlib import closing
import re
from pathlib import Path
from playwright.sync_api import expect
from test_app_server import route_call, unroute_call


def run_collections(page, base: str) -> None:
    page.goto(f'{base}/?transport=mock#/collections')
    expect(page.get_by_role('heading', name='Collections', level=1)).to_be_visible()
    # Permanent live regions exist before the action and survive failures.
    result = page.get_by_role('status', name='Collection result')
    expect(result).to_have_count(1)
    result_node = result.element_handle()
    page.get_by_label('Collection name', exact=True).fill('Projects')
    page.get_by_role('button', name='Create collection', exact=True).click()
    expect(result).to_contain_text('Collection created')
    assert result_node.evaluate("el => el.isConnected && el.textContent.includes('Collection created')")
    expect(page.get_by_role('button', name='Create collection', exact=True)).to_be_focused()
    expect(page.get_by_role('heading', name='Projects', level=2, exact=True)).to_be_visible()
    page.get_by_label('Collection name', exact=True).fill('Projects')
    page.get_by_role('button', name='Create collection', exact=True).click()
    expect(page.get_by_role('alert', name='Collection error')).to_contain_text('already exists')
    page.get_by_role('link', name='Explorer', exact=True).click()
    expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_focused()
    page.get_by_role('button', name='Inspect ', exact=False).first.click()
    inspector = page.get_by_role('region', name='Node inspector')
    expect(inspector.get_by_role('heading', level=2)).to_be_focused()
    expect(inspector.locator('p').filter(has_text='Logical size')).to_contain_text(' · ')
    membership = inspector.get_by_role('status', name='Membership result')
    expect(membership).to_have_count(1)
    membership_node = membership.element_handle()
    page.get_by_role('button', name='Add to collection', exact=True).click()
    expect(membership).to_contain_text('Added to collection')
    assert membership_node.evaluate("el => el.isConnected && el.textContent.includes('Added to collection')")
    page.get_by_role('link', name='Collections', exact=True).click()
    expect(page.get_by_role('heading', name='Collections', level=1)).to_be_focused()
    expect(page.locator('main table tbody th[scope=row]')).to_have_count(1)
    expect(page.locator('main table tbody')).to_contain_text('Resolved')
    expect(page.get_by_text('Showing 1 of 1 members', exact=True)).to_be_visible()
    # A revoked object keeps its row with explicit unknowns.
    page.get_by_role('link', name='Grants & health', exact=True).click()
    page.get_by_role('button', name=re.compile('^Revoke')).first.click()
    page.get_by_role('group', name=re.compile('Revoke root access')).get_by_role('button', name='Revoke', exact=True).click()
    expect(page.get_by_role('status').filter(has_text='Revoked')).to_be_visible()
    page.get_by_role('link', name='Collections', exact=True).click()
    expect(page.locator('main table tbody')).to_contain_text('Root revoked')
    expect(page.locator('main table tbody')).to_contain_text('unknown')
    remove = page.get_by_role('button', name=re.compile('^Remove member '))
    remove.focus()
    remove.press('Enter')
    expect(result).to_contain_text('Member removed')
    expect(page.get_by_text('Showing 0 of 0 members', exact=True)).to_be_visible()
    expect(page.get_by_text('Showing 0 of 0 members', exact=True)).to_be_focused()
    print('PASS collections: create, duplicate refusal, inspector add, separator, revoked removal by keyboard and restored focus')

    page.goto(f'{base}/?transport=mock#/collections')
    page.get_by_label('Collection name', exact=True).fill('Batch papers')
    page.get_by_role('button', name='Create collection', exact=True).click()
    expect(result).to_contain_text('Collection created')
    page.get_by_role('link', name='Explorer', exact=True).click()
    choices = page.locator('main table tbody').get_by_role('checkbox')
    choices.nth(0).check()
    choices.nth(1).check()
    page.get_by_role('button', name='Add selected to collection', exact=True).click()
    expect(page.get_by_role('status', name='Selected rows result', exact=True)).to_contain_text('Saved selection of 2 rows')
    page.get_by_role('link', name='Collections', exact=True).click()
    choices = page.locator('main table tbody').get_by_role('checkbox')
    expect(choices).to_have_count(2)
    for index in [0, 1]:
        choices.nth(index).focus()
        choices.nth(index).press('Space')
    selected = page.get_by_role('status', name='Selected collection members', exact=True)
    expect(selected).to_contain_text('2 selected')
    clear = page.get_by_role('button', name='Clear member selection', exact=True)
    clear.focus()
    clear.press('Enter')
    expect(selected).to_contain_text('0 selected')
    expect(page.get_by_role('button', name='Remove selected members', exact=True)).to_be_disabled()
    choices.nth(0).check()
    choices.nth(1).check()
    remove_selected = page.get_by_role('button', name='Remove selected members', exact=True)
    remove_selected.focus()
    remove_selected.press('Enter')
    expect(result).to_contain_text('Removal saved for the selection of 2 members')
    expect(page.get_by_text('Showing 0 of 0 members', exact=True)).to_be_focused()
    expect(selected).to_contain_text('0 selected')
    print('PASS collection curation mock: selected add/remove, keyboard selection, clear and focus')


def run_live_collections(page, token: str, state: Path, check) -> None:
    # A single invented catalogue node exercises the real service without a disk scan or content read.
    # These are disposable synthetic DB rows, not imported inventory and not host observations.
    with closing(sqlite3.connect(state / 'state.db')) as db:
        db.execute("INSERT INTO root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(1,'collection-fixture',zeroblob(16),'Synthetic collection root','fixture','fixture','active',0)")
        db.execute('UPDATE revision SET state_rev=state_rev+1')
        db.commit()
    with closing(sqlite3.connect(state / 'catalog.db')) as db:
        db.executescript("""
        INSERT INTO volume(id,volume_key,display_name,filesystem,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES(1,'collection-fixture','Synthetic','NTFS','{}','{}','{}',1,0);
        INSERT INTO root(id,grant_id,volume_id,root_file_id,generation,state) VALUES(1,1,1,zeroblob(16),1,'complete');
        INSERT INTO dir(id,root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,created_ft,listing_state,born_run,seen_run,sub_files,sub_logical,sub_allocated,sub_complete) VALUES(1,1,NULL,'Synthetic collection root',zeroblob(16),'native_file_id_128','listed',0,0,123,'complete',1,1,1,17,17,1);
        INSERT INTO file(id,dir_id,name,file_id,logical,allocated,created_ft,attrs,born_run,seen_run) VALUES(1,1,'report.txt',CAST('collection-file1' AS BLOB),17,17,123,0,1,1);
        UPDATE revision SET catalog_rev=catalog_rev+1;
        """)
    def call(command, payload, request_id, expected=None):
        req = {'protocol': 'loomward/3', 'request_id': request_id, 'command': command, 'payload': payload}
        if expected is not None:
            req['expected_state_rev'] = expected
        return page.evaluate("""async ([token, req]) => (await (await fetch('/api/v3/call', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Loomward-Token': token }, body: JSON.stringify(req) })).json())""", [token, req])
    created = call('collections.create', {'name': 'Live projects'}, 'collections_live_create')
    check(created['ok'], 'live collections: create')
    check(call('collections.create', {'name': 'Live projects'}, 'collections_live_create') == created, 'live collections: create replay returns the original envelope')
    duplicate = call('collections.create', {'name': 'Live projects'}, 'collections_live_duplicate')
    check(not duplicate['ok'] and duplicate['error']['code'] == 'invalid_request' and duplicate['error']['detail']['existing_id'] == created['result']['collection_id'], 'live collections: duplicate name includes existing_id')
    found = call('search.query', {'root_id': None, 'text': 'report', 'extension': None, 'min_bytes': None, 'max_bytes': None, 'modified_after': None, 'modified_before': None, 'kind': 'file', 'limit': 1, 'cursor': None}, 'collections_live_search')
    check(found['ok'] and len(found['result']['hits']) == 1 and found['result']['index_state'] == 'current', 'live collections: service seals the synthetic node')
    collection_id = created['result']['collection_id']
    read = call('collections.members', {'collection_id': collection_id, 'limit': 1, 'cursor': None}, 'collections_live_read')
    added = call('collections.update_members', {'collection_id': collection_id, 'add': [found['result']['hits'][0]['row']['node_id']], 'remove': []}, 'collections_live_add', read['meta']['state_rev'])
    check(added['ok'] and added['result']['member_count'] == 1, 'live collections: add with expected_state_rev')
    page_result = call('collections.members', {'collection_id': collection_id, 'limit': 200, 'cursor': None}, 'collections_live_members')
    check(page_result['ok'] and page_result['result']['items'][0]['status'] == 'resolved' and page_result['result']['items'][0]['entry']['name'] == 'report.txt', 'live collections: list durable members through the real HTTP service')
    page.get_by_role('link', name='Collections', exact=True).click()
    expect(page.locator('main table tbody')).to_contain_text('report.txt')
    expect(page.locator('main table tbody')).to_contain_text('Resolved')
    with closing(sqlite3.connect(state / 'catalog.db')) as db:
        db.execute('DELETE FROM file WHERE id=1')
        db.execute('UPDATE revision SET catalog_rev=catalog_rev+1')
        db.commit()
    page.get_by_role('button', name='Refresh collections', exact=True).click()
    expect(page.locator('main table tbody')).to_contain_text('Object unavailable')
    page.get_by_role('button', name=re.compile('^Remove member ')).click()
    expect(page.get_by_role('status', name='Collection result')).to_contain_text('Member removed')
    expect(page.get_by_text('Showing 0 of 0 members', exact=True)).to_be_visible()
    print('PASS live collections: production app renders the real collection')

    # Extend the same invented catalogue; no native root or owner file is opened.
    with closing(sqlite3.connect(state / 'catalog.db')) as db:
        db.executemany('INSERT INTO file(id,dir_id,name,file_id,logical,allocated,created_ft,attrs,born_run,seen_run) VALUES(?,1,?,?,17,17,123,0,1,1)',
                       [(i, f'curation-{i:03}.txt', f'curation-{i:07}'.encode()) for i in range(2, 54)])
        db.execute('UPDATE revision SET catalog_rev=catalog_rev+1')
        db.commit()
    found = call('search.query', {'root_id': None, 'text': 'curation', 'extension': None, 'min_bytes': None, 'max_bytes': None, 'modified_after': None, 'modified_before': None, 'kind': 'file', 'limit': 100, 'cursor': None}, 'curation_nodes')
    check(found['ok'] and len(found['result']['hits']) == 52, 'curation: 52 synthetic current identities')
    ids = [hit['row']['node_id'] for hit in sorted(found['result']['hits'], key=lambda hit: hit['row']['name'])]
    other = call('collections.create', {'name': 'Keep these members'}, 'curation_other')['result']['collection_id']
    for index, target in enumerate([collection_id, other]):
        rev = call('collections.members', {'collection_id': target, 'limit': 1, 'cursor': None}, f'curation_rev_{index}')['meta']['state_rev']
        check(call('collections.update_members', {'collection_id': target, 'add': ids, 'remove': []}, f'curation_seed_{index}', rev)['ok'], 'curation: populate each collection')
    with closing(sqlite3.connect(state / 'catalog.db')) as db:
        db.execute('DELETE FROM file WHERE id=2')
        db.execute('UPDATE revision SET catalog_rev=catalog_rev+1')
        db.commit()
    payload = {'collection_id': collection_id, 'limit': 200, 'cursor': None}
    before = call('collections.members', payload, 'curation_before')['result']
    check(before['total'] == 52 and any(m['status'] == 'unresolved' for m in before['items']), 'curation: unavailable membership remains selectable')
    grants = call('grants.list', {}, 'curation_grants_before')['result']
    unknown_id = next(m['member_id'] for m in before['items'] if m['status'] == 'unresolved')
    # Durable IDs, rather than the search order, determine the paged selection.
    selected_ids = [before['items'][0]['member_id'], before['items'][-1]['member_id']]
    check(unknown_id in selected_ids, 'curation: selected batch includes the unresolved object')
    refresh = page.get_by_role('button', name='Refresh collections', exact=True)
    rows = page.locator('main table tbody tr')
    selected = page.get_by_role('status', name='Selected collection members', exact=True)
    result = page.get_by_role('status', name='Collection result', exact=True)
    remove_selected = page.get_by_role('button', name='Remove selected members', exact=True)

    def select_ends():
        refresh.click()
        expect(rows).to_have_count(50)
        first = rows.first.get_by_role('checkbox')
        first.focus()
        first.press('Space')
        more = page.get_by_role('button', name='Load more members', exact=True)
        more.focus()
        more.press('Enter')
        expect(rows).to_have_count(52)
        expect(first).to_be_checked()
        last = rows.last.get_by_role('checkbox')
        last.focus()
        last.press('Space')
        expect(selected).to_contain_text('2 selected')

    select_ends()
    check(call('collections.create', {'name': 'Concurrent curation change'}, 'curation_concurrent')['ok'], 'curation: unrelated preference changes observed revision')
    remove_selected.click()
    expect(page.get_by_role('alert', name='Collection error', exact=True)).to_contain_text('state.db changed')
    expect(selected).to_contain_text('0 selected')
    expect(remove_selected).to_be_disabled()
    check(call('collections.members', payload, 'curation_after_stale')['result'] == before, 'curation: stale batch changed no memberships')
    select_ends()
    captures = Path(__file__).resolve().parents[1] / '.loomward'
    captures.mkdir(exist_ok=True)
    for width in [1360, 390]:
        page.set_viewport_size({'width': width, 'height': 900})
        page.evaluate('scrollTo(0, 0)')
        check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'curation: controls fit {width}px')
        # Reuse the existing hosted artifact glob; this is a Collections capture.
        page.screenshot(path=str(captures / f'search-root-collection-curation-{width}.png'), full_page=True)
    page.set_viewport_size({'width': 1360, 'height': 900})
    remove_selected.focus()
    remove_selected.press('Enter')
    expect(result).to_contain_text('Removal saved for the selection of 2 members')
    expect(page.get_by_text('Showing 50 of 50 members', exact=True)).to_be_focused()
    after = call('collections.members', payload, 'curation_after')['result']
    check([m['member_id'] for m in after['items']] == [m['member_id'] for m in before['items'] if m['member_id'] not in selected_ids], 'curation: only the explicit selected set was removed')
    check(call('collections.members', {**payload, 'collection_id': other}, 'curation_other_after')['result']['total'] == 52, 'curation: the other collection is unchanged')

    # Commit through the real service, then hide its success behind a timeout envelope.
    uncertain_writes = []

    def timeout_after_commit(route):
        request = route.request.post_data_json
        if request['command'] != 'collections.update_members':
            route.continue_()
            return
        response = route.fetch().json()
        check(response['ok'], 'curation: uncertain write actually committed')
        uncertain_writes.append(request)
        route.fulfill(json={'protocol': 'loomward/3', 'request_id': request['request_id'], 'ok': False,
                            'error': {'code': 'deadline_exceeded', 'message': 'Outcome unknown', 'retryable': True, 'detail': None}})

    route_call(page, timeout_after_commit)
    try:
        rows.first.get_by_role('checkbox').check()
        remove_selected.click()
        expect(result).to_contain_text('may already have been saved')
        expect(remove_selected).to_be_disabled()
        expect(selected).to_contain_text('0 selected')
        refresh.click()
        expect(page.get_by_text('Showing 49 of 49 members', exact=True)).to_be_visible()
        expect(result).to_contain_text('may already have been saved')
        check(len(uncertain_writes) == 1, 'curation: reread does not replay an uncertain write')
    finally:
        unroute_call(page, timeout_after_commit)
    check(call('grants.list', {}, 'curation_grants_after')['result'] == grants, 'curation: grants unchanged')
    with closing(sqlite3.connect(state / 'state.db')) as db:
        db.execute("INSERT INTO collection(name,kind,created_at_ns) VALUES('Read-only label view','label_view',0)")
        db.execute('UPDATE revision SET state_rev=state_rev+1')
        db.commit()
    refresh.click()
    # The wrapping label also contains every option's text; get_by_label(exact=True)
    # therefore matches no select. Its accessible combobox name is just Collection.
    picker = page.get_by_role('combobox', name='Collection', exact=True)
    expect(picker).to_have_count(1)
    expect(picker.locator('option').filter(has_text='Read-only label view')).to_have_count(1)
    picker.select_option(label='Read-only label view')
    expect(remove_selected).to_have_count(0)
    expect(rows.get_by_role('checkbox')).to_have_count(0)
    expect(page.get_by_role('button', name=re.compile('^Remove member '))).to_have_count(0)
    expect(page.get_by_text('Showing 0 of 0 members', exact=True)).to_be_visible()
    expect(refresh).to_have_attribute('aria-disabled', 'false')
    picker.select_option(label='Live projects')
    expect(page.get_by_text('Showing 49 of 49 members', exact=True)).to_be_visible()
    expect(rows.get_by_role('checkbox')).to_have_count(49)
    print('PASS live collection curation: multi-page keyboard selection, unavailable member, atomic stale refusal, exact removal, other collection/grants unchanged, committed unknown outcome without retry, and label-view protection')
    run_live_collection_search(page, state, check, call, other)


def run_live_collection_search(page, state: Path, check, call, collection_id: str) -> None:
    """Production Explorer over the disposable native collection; no filesystem inventory."""
    subset = call('collections.create', {'name': 'Retained search subset'}, 'collection_search_subset')['result']['collection_id']
    filters = {'collection_id': collection_id, 'text': 'CURATION', 'extension': None, 'min_bytes': None,
               'max_bytes': None, 'modified_after': None, 'modified_before': None, 'kind': 'any', 'limit': 50, 'cursor': None}
    initial = call('collections.search', filters, 'collection_search_initial')
    check(initial['ok'] and len(initial['result']['hits']) == 50 and initial['result']['total_members'] == 52,
          'collection search: direct scope before native page limit, durable unavailable membership included in total')
    check(initial['result']['unavailable_members'] == 1, 'collection search: unavailable member counted without metadata')
    next_page = call('collections.search', {**filters, 'cursor': initial['result']['next_cursor']}, 'collection_search_second')
    check(next_page['ok'] and len(next_page['result']['hits']) == 1 and next_page['result']['next_cursor'] is None,
          'collection search: native cursor reaches the last match')
    changed = call('collections.search', {**filters, 'text': 'different', 'cursor': initial['result']['next_cursor']}, 'collection_search_wrong_query')
    check(not changed['ok'] and changed['error']['code'] == 'invalid_request', 'collection search: cursor refuses another query')
    # Empty and paused views render a placeholder row; count actual search hits only.
    rows = page.locator('main table tbody tr:has(button[data-inspect])')
    page.get_by_role('link', name='Explorer', exact=True).click()
    scope = page.get_by_role('combobox', name='Search scope', exact=True)
    scope.select_option(value=collection_id)
    search = page.get_by_role('searchbox', name='Search names', exact=True)
    search.fill('curation')
    search.press('Enter')
    expect(rows).to_have_count(50)
    expect(page.get_by_label('Applied search scope', exact=True)).to_contain_text('direct members only')
    expect(page.get_by_label('Collection search coverage', exact=True)).to_contain_text('1 unavailable')
    # Keyboard inspection uses the same production inspector as root search.
    search.focus()
    search.press('ArrowDown')
    expect(rows.first.get_by_role('button', name=re.compile('^Inspect '))).to_be_focused()
    page.keyboard.press('Enter')
    expect(page.get_by_role('region', name='Node inspector')).to_be_visible()
    rows.first.get_by_role('checkbox').focus()
    page.keyboard.press('Space')
    # A changed draft scope must not alter the submitted page cursor.
    scope.select_option(value=subset)
    more = page.get_by_role('button', name='Load more', exact=True)
    more.focus()
    more.press('Enter')
    expect(rows).to_have_count(51)
    expect(rows.first.get_by_role('checkbox')).to_be_checked()
    rows.last.get_by_role('checkbox').check()
    selected_ids = [initial['result']['hits'][0]['row']['node_id'], next_page['result']['hits'][0]['row']['node_id']]
    page.get_by_role('combobox', name='Collection for selected rows', exact=True).select_option(value=subset)
    page.get_by_role('button', name='Add selected to collection', exact=True).click()
    expect(page.get_by_role('status', name='Selected rows result', exact=True)).to_contain_text('Saved selection of 2 rows')
    saved = call('collections.members', {'collection_id': subset, 'limit': 100, 'cursor': None}, 'collection_search_saved')
    check(saved['ok'] and sorted(m['entry']['node_id'] for m in saved['result']['items']) == sorted(selected_ids),
          'collection search: explicit cross-page subset retained through existing revision-fenced action')
    page.get_by_role('link', name='Collections', exact=True).click()
    page.get_by_role('combobox', name='Collection', exact=True).select_option(value=subset)
    page.get_by_role('link', name='Search direct members', exact=True).click()
    expect(page.get_by_label('Applied search scope', exact=True)).to_contain_text('Retained search subset')
    expect(rows).to_have_count(2)
    page.reload()
    expect(rows).to_have_count(2)
    expect(page.get_by_label('Applied search scope', exact=True)).to_contain_text('Retained search subset')
    for width in [1360, 390]:
        page.set_viewport_size({'width': width, 'height': 900})
        check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'collection search: controls fit {width}px')
        captures = Path(__file__).resolve().parents[1] / '.loomward'
        captures.mkdir(exist_ok=True)
        page.screenshot(path=str(captures / f'search-root-collection-search-{width}.png'), full_page=True)
    page.set_viewport_size({'width': 1360, 'height': 900})

    # Capture a valid native response, then return it after a newer submitted request. The client
    # request identity/generation must reject the cancelled old page even if it reaches the browser.
    held = []
    def hold_search(route):
        request = route.request.post_data_json
        if request['command'] == 'collections.search' and request['payload']['text'] == 'curation':
            held.append((route, route.fetch().json()))
        else:
            route.continue_()
    route_call(page, hold_search)
    try:
        search.fill('curation')
        with page.expect_request(lambda request: request.method == 'POST' and request.post_data_json.get('command') == 'collections.search'):
            search.press('Enter')
        # A DOM action yields to request routing; the pending first request is deliberately held.
        search.fill('no-literal-match')
        search.press('Enter')
        expect(page.get_by_label('Collection search coverage', exact=True)).to_contain_text('0 matches loaded')
        for route, response in held:
            route.fulfill(json=response)
        held.clear()
        expect(rows).to_have_count(0)
        expect(page.get_by_label('Collection search coverage', exact=True)).to_contain_text('0 matches loaded')
    finally:
        for route, _ in held:
            route.abort()
        unroute_call(page, hold_search)

    # A transport loss unmounts the view. Its restored collection search requires a new explicit
    # scope choice; the durable collection ID in the route is not a session authorization.
    dropped = []
    def disconnect_once(route):
        if route.request.post_data_json['command'] == 'collections.search' and not dropped:
            dropped.append(True)
            route.abort('failed')
        else:
            route.continue_()
    route_call(page, disconnect_once)
    try:
        search.fill('curation')
        search.press('Enter')
        expect(page.get_by_label('Applied search scope', exact=True)).to_contain_text('Search paused', timeout=15000)
        expect(rows).to_have_count(0)
        scope.select_option(value=subset)
        search.fill('curation')
        search.press('Enter')
        expect(rows).to_have_count(2)
    finally:
        unroute_call(page, disconnect_once)

    def old_engine(route):
        response = route.fetch().json()
        if route.request.post_data_json['command'] == 'session.hello' and response.get('ok'):
            response['result']['features'].pop('collection_search', None)
        route.fulfill(json=response)
    route_call(page, old_engine)
    try:
        page.reload()
        expect(scope).to_be_visible()
        expect(scope.locator('optgroup')).to_have_count(0)
        expect(page.get_by_label('Applied search scope', exact=True)).to_contain_text('Search paused')
        page.get_by_role('link', name='Collections', exact=True).click()
        expect(page.get_by_role('link', name='Search direct members', exact=True)).to_have_count(0)
    finally:
        unroute_call(page, old_engine)
    page.reload()
    print('PASS native collection search: scope/filter pages, honest unavailable counts, inspection, keyboard cross-page subset retention/reload, late cancellation, reconnect refusal, old-engine absence and narrow layout')
