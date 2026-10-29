"""Production HTTP/Svelte discovery over invented catalogue rows; no source scan or model."""
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3

from playwright.sync_api import expect


def live_projects(page, token, state, check, live_call):
    # Continue the disposable collection fixture. Its granted display path does not exist.
    with closing(sqlite3.connect(state / 'catalog.db')) as db:
        for ident, parent, name in [(101, 1, 'Synthetic project'), (102, 101, 'Nested project'), (103, 1, 'Lookalikes'), (104, 1, 'Excluded project')]:
            db.execute("INSERT INTO dir(id,root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,created_ft,listing_state,born_run,seen_run,sub_complete) VALUES(?,1,?,?,?,'native_file_id_128','listed',1,16,123,'complete',1,1,1)", (ident, parent, name, ident.to_bytes(16, 'little')))
        for owner, name in [(101, 'Cargo.toml'), (101, 'package.json'), (102, 'pyproject.toml'), (103, 'cargo.toml'), (103, '.csproj'), (104, 'Cargo.toml')]:
            db.execute('INSERT INTO file(dir_id,name,logical,allocated,attrs,born_run,seen_run) VALUES(?,?,1,1,32,1,1)', (owner, name))
        db.execute("UPDATE dir SET listing_state='excluded' WHERE id=104")
        db.execute('UPDATE revision SET catalog_rev=catalog_rev+1')
        db.commit()
    root = live_call(page, token, 'roots.list', {})['result']['roots'][0]['root_id']
    def snapshot():
        dumps = []
        for name in ['catalog.db', 'state.db']:
            with closing(sqlite3.connect(state / name)) as db:
                dumps.append(list(db.iterdump()))
        return dumps
    before = snapshot()
    response = live_call(page, token, 'projects.find', {'root_id': root, 'limit': 50, 'cursor': None})
    check(response['ok'], 'projects: real service answers the owner command')
    result = response['result']
    check([p['name'] for p in result['candidates']] == ['Synthetic project', 'Nested project'], 'projects: no-output and nested projects, consolidated markers, exact lookalikes and exclusion shield')
    check(result['partial'] and snapshot() == before, 'projects: excluded coverage stays partial and discovery writes neither database')
    requests = []
    def observe(req):
        if req.url.endswith('/api/v3/call') and req.post_data:
            requests.append(json.loads(req.post_data))
    page.on('request', observe)
    try:
        page.evaluate("location.hash = '#/collections'")
        region = page.get_by_role('region', name='Projects found here', exact=True)
        expect(region.get_by_role('button', name='Find projects', exact=True)).to_be_enabled()
        check(not any(r['command'] == 'projects.find' for r in requests), 'projects: entering Collections does not discover automatically')
        region.get_by_role('button', name='Find projects', exact=True).click()
        expect(region.get_by_role('article')).to_have_count(2)
        expect(region).to_contain_text('Projects may be missing')
        candidate = region.get_by_role('article', name='Synthetic project', exact=True)
        expect(candidate).to_contain_text('Cargo.toml')
        expect(candidate).to_contain_text('package.json')
        evidence = Path(os.environ.get('LOOMWARD_PROJECTS_EVIDENCE', str(Path(__file__).resolve().parents[1] / 'target/project-ui-evidence')))
        evidence.mkdir(parents=True, exist_ok=True)
        for width in [1440, 390]:
            page.set_viewport_size({'width': width, 'height': 900})
            check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'projects: fits {width}px')
            page.screenshot(path=str(evidence / f'projects-{width}.png'), full_page=True)
        candidate.get_by_role('link', name='Inspect in Explorer').click()
        expect(page.get_by_role('heading', name='Explorer', exact=True)).to_be_visible()
        expect(page.locator('main')).to_contain_text('Synthetic project')
        expect(page.get_by_role('button', name='Add to collection', exact=True)).to_be_visible()
        page.get_by_role('button', name='Add to collection', exact=True).click()
        expect(page.get_by_role('status', name='Membership result')).to_contain_text('Added to collection')
        adds = [r for r in requests if r['command'] == 'collections.update_members']
        check(len(adds) == 1 and adds[0].get('expected_generation') == result['generation'] and 'expected_state_rev' in adds[0], 'projects: Explorer uses explicit add with both revision guards')
        page.evaluate("location.hash = '#/collections'")
        region.get_by_role('button', name='Find projects', exact=True).click()
        candidate.get_by_role('button', name='Choose collection').click()
        expect(candidate.get_by_role('button', name='Add to collection', exact=True)).to_be_enabled()
        page.get_by_label('Collection name', exact=True).fill('New project collection')
        page.get_by_role('button', name='Create collection', exact=True).click()
        expect(page.get_by_role('status', name='Collection result')).to_contain_text('Collection created')
        expect(candidate.get_by_role('combobox').locator('option').filter(has_text='New project collection')).to_have_count(1)
        with closing(sqlite3.connect(state / 'catalog.db')) as db:
            db.execute('UPDATE root SET generation=generation+1 WHERE id=1')
            db.execute('UPDATE revision SET catalog_rev=catalog_rev+1')
            db.commit()
        candidate.get_by_role('button', name='Add to collection', exact=True).click()
        expect(candidate.get_by_role('alert')).to_contain_text('changed')
        # Partial empty must never be described as proof that no projects exist.
        with closing(sqlite3.connect(state / 'catalog.db')) as db:
            db.execute('DELETE FROM file WHERE dir_id>=101')
            db.execute('UPDATE revision SET catalog_rev=catalog_rev+1')
            db.commit()
        region.get_by_role('button', name='Find projects', exact=True).click()
        expect(region).to_contain_text('This does not establish that no projects exist')
        with closing(sqlite3.connect(state / 'catalog.db')) as db:
            db.execute("UPDATE dir SET listing_state='complete' WHERE id=104")
            db.execute('UPDATE revision SET catalog_rev=catalog_rev+1')
            db.commit()
        region.get_by_role('button', name='Find projects', exact=True).click()
        expect(region).to_contain_text('No recognised project markers are recorded')
        (evidence / 'projects-native.json').write_text(json.dumps({'dataset': 'synthetic', 'production_http': True,
            'filesystem_observation': False, 'discovery_database_unchanged': True, 'generation_guard': True,
            'explicit_membership': True, 'widths': [1440, 390], 'partial_and_empty': True}, indent=2) + '\n', encoding='utf-8')
    finally:
        page.remove_listener('request', observe)
        page.set_viewport_size({'width': 1360, 'height': 900})
