"""Synthetic collection UI and real loopback service checks; no native root opens."""
from __future__ import annotations
import sqlite3
from contextlib import closing
import re
from pathlib import Path
from playwright.sync_api import expect


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
    found = call('search.query', {'root_id': None, 'text': 'curation', 'extension': None, 'min_bytes': None, 'max_bytes': None, 'modified_after': None, 'modified_before': None, 'kind': 'file', 'limit': 200, 'cursor': None}, 'curation_nodes')
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

    page.route('**/api/v3/call', timeout_after_commit)
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
        page.unroute('**/api/v3/call', timeout_after_commit)
    check(call('grants.list', {}, 'curation_grants_after')['result'] == grants, 'curation: grants unchanged')
    with closing(sqlite3.connect(state / 'state.db')) as db:
        db.execute("INSERT INTO collection(name,kind,created_at_ns) VALUES('Read-only label view','label_view',0)")
        db.execute('UPDATE revision SET state_rev=state_rev+1')
        db.commit()
    refresh.click()
    picker = page.get_by_label('Collection', exact=True)
    expect(picker.locator('option').filter(has_text='Read-only label view')).to_have_count(1)
    picker.select_option(label='Read-only label view')
    expect(remove_selected).to_have_count(0)
    expect(rows.get_by_role('checkbox')).to_have_count(0)
    expect(page.get_by_role('button', name=re.compile('^Remove member '))).to_have_count(0)
    print('PASS live collection curation: multi-page keyboard selection, unavailable member, atomic stale refusal, exact removal, other collection/grants unchanged, committed unknown outcome without retry, and label-view protection')
