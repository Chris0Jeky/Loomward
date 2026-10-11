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
    expect(page.get_by_role('heading', name='Projects', level=2)).to_be_visible()
    page.get_by_label('Collection name', exact=True).fill('Projects')
    page.get_by_role('button', name='Create collection', exact=True).click()
    expect(page.locator('main [role=alert]')).to_contain_text('already exists')
    page.get_by_role('link', name='Explorer', exact=True).click()
    expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_focused()
    page.get_by_role('button', name='Inspect ', exact=False).first.click()
    inspector = page.get_by_role('region', name='Node inspector')
    expect(inspector.get_by_role('heading', level=2)).to_be_focused()
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
    print('PASS collections: create, duplicate refusal, inspector add, focus, row headers, persistent results and revoked member')


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
    found = call('search.query', {'root_id': None, 'text': 'report', 'extension': None, 'min_bytes': None, 'kind': 'file', 'limit': 1, 'cursor': None}, 'collections_live_search')
    check(found['ok'] and len(found['result']['items']) == 1, 'live collections: service seals the synthetic node')
    collection_id = created['result']['collection_id']
    read = call('collections.members', {'collection_id': collection_id, 'limit': 1, 'cursor': None}, 'collections_live_read')
    added = call('collections.update_members', {'collection_id': collection_id, 'add': [found['result']['items'][0]['node_id']], 'remove': []}, 'collections_live_add', read['meta']['state_rev'])
    check(added['ok'] and added['result']['member_count'] == 1, 'live collections: add with expected_state_rev')
    page_result = call('collections.members', {'collection_id': collection_id, 'limit': 200, 'cursor': None}, 'collections_live_members')
    check(page_result['ok'] and page_result['result']['items'][0]['status'] == 'resolved' and page_result['result']['items'][0]['entry']['name'] == 'report.txt', 'live collections: list durable members through the real HTTP service')
    page.get_by_role('link', name='Collections', exact=True).click()
    expect(page.locator('main table tbody')).to_contain_text('report.txt')
    expect(page.locator('main table tbody')).to_contain_text('Resolved')
    print('PASS live collections: production app renders the real collection')
