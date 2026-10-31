"""Durable destination headroom through the real HTTP service and Tiers controls."""
from contextlib import closing
import json
from pathlib import Path
import re
import sqlite3
import subprocess
import tempfile

from playwright.sync_api import expect


def run_live_reserve(browser, exe: Path, dist: Path, check, call, evidence: Path | None = None) -> None:
    gib = 1 << 30
    with tempfile.TemporaryDirectory(prefix='lw-live-reserve-') as temporary:
        state = Path(temporary) / 'state'
        original = Path(temporary) / 'Reserve group' / 'synthetic.bin'
        original.parent.mkdir()
        original.write_bytes(b'untouched synthetic original')
        before = (original.read_bytes(), original.stat().st_mtime_ns)
        proc = None
        context = None
        errors = []

        def start():
            nonlocal proc, context
            proc = subprocess.Popen([str(exe), '--state-dir', str(state), '--static', str(dist)], stdout=subprocess.PIPE, text=True)
            url = proc.stdout.readline().strip()
            check('/#token=' in url, 'reserve: native synthetic service started')
            context = browser.new_context(viewport={'width': 1440, 'height': 1000})
            page = context.new_page()
            page.set_default_timeout(10000)
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.goto(url)
            expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
            return page, url.split('/#token=')[1]

        def stop():
            if context:
                context.close()
            if proc and proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=10)

        def grants():
            with closing(sqlite3.connect(state / 'state.db')) as db:
                return db.execute('SELECT * FROM root_grant ORDER BY id').fetchall()

        def save_reserve(page, value):
            page.get_by_label('Keep at least free for Reserve target', exact=True).fill(value)
            page.get_by_role('button', name='Save reserve for Reserve target', exact=True).click()
            expect(page.locator('.vol').filter(has=page.get_by_role('heading', name='Reserve target', exact=True))).to_contain_text(f'({int(value) * gib} bytes)')
            expect(page.get_by_role('button', name='Save reserve for Reserve target', exact=True)).to_be_enabled()

        def simulate(page):
            page.get_by_label('Assumed heat for Reserve group').select_option('0.1')
            page.get_by_label('Assume not pinned, active or protected for Reserve group').check()
            page.get_by_label('Assume no move in the past 7 days for Reserve group').check()
            page.get_by_label('Space freed counted from').select_option('entry_allocation_whatif')
            page.get_by_label('Keep these scenarios').check()
            def assumed_reply(response):
                if not response.url.endswith('/api/v3/call') or response.request.method != 'POST':
                    return False
                request = response.request.post_data_json
                return request.get('command') == 'placement.simulate' and request['payload']['heat_policy'] == 'assumed_only'
            with page.expect_response(assumed_reply) as response:
                page.get_by_role('button', name='Simulate', exact=True).click()
            envelope = response.value.json()
            check(envelope['ok'], 'reserve: app simulation succeeds over native HTTP')
            expect(page.get_by_role('heading', name='Placement plan', exact=True)).to_be_visible()
            return envelope['result']

        try:
            page, token = start()
            with closing(sqlite3.connect(state / 'state.db')) as db, db:
                for row, key in [(1, 'reserve-source'), (2, 'reserve-target')]:
                    display_path = str(original.parent) if row == 1 else 'Synthetic destination'
                    db.execute("INSERT INTO root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(?,?,?,?,'fixture','fixture','active',0)", [row, key, bytes([row] * 16), display_path])
                db.execute('UPDATE revision SET state_rev=state_rev+1')
            with closing(sqlite3.connect(state / 'catalog.db')) as db, db:
                for row, key, name, free in [(1, 'reserve-source', 'Reserve source', 10), (2, 'reserve-target', 'Reserve target', 100)]:
                    physics = json.dumps({'space': {'capacity_bytes': str(200 * gib), 'free_bytes': str(free * gib), 'read_only': False}})
                    db.execute("INSERT INTO volume(id,volume_key,display_name,bytes_per_cluster,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES(?,?,?,4096,'{}',?,'{}',1,0)", [row, key, name, physics])
                    db.execute("INSERT INTO root(id,grant_id,volume_id,root_file_id,generation,state) VALUES(?,?,?,?,1,'complete')", [row, row, row, bytes([row] * 16)])
                db.execute("INSERT INTO dir(id,root_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,sub_files,sub_logical,sub_allocated,sub_complete) VALUES(1,1,'Reserve group',?,'native_file_id_128','listed',0,16,'complete',1,1,1,?,?,1)", [bytes([1] * 16), 30 * gib, 30 * gib])
                db.execute("INSERT INTO file(id,dir_id,name,file_id,logical,allocated,attrs,born_run,seen_run) VALUES(1,1,'synthetic.bin',?,?,?,0,1,1)", [bytes([3] * 16), 30 * gib, 30 * gib])
                db.execute('UPDATE revision SET catalog_rev=1')
            original_grants = grants()
            page.evaluate("location.hash = '#/tiers'")
            expect(page.get_by_role('heading', name='Tiers', level=1)).to_be_visible()
            page.get_by_label('Declared tier for Reserve source').select_option('0')
            expect(page.get_by_label('Declared tier for Reserve target')).to_be_enabled()
            page.get_by_label('Declared tier for Reserve target').select_option('1')
            expect(page.get_by_role('button', name='Save reserve for Reserve target', exact=True)).to_be_enabled()
            save_reserve(page, '80')
            stop()
            context = proc = None
            page, token = start()
            page.evaluate("location.hash = '#/tiers'")
            expect(page.get_by_label('Keep at least free for Reserve target', exact=True)).to_have_value('80')
            check(grants() == original_grants, 'reserve: restart retains original grants unchanged')
            blocked = simulate(page)
            check(not blocked['satisfied'] and blocked['shortfall_bytes'] == str(30 * gib) and not blocked['proposals'], 'reserve: 100 GiB free minus 80 GiB reserve refuses the 30 GiB candidate')
            check(blocked['search']['eligible_groups'] == 1 and not blocked['pre_rejected'], 'reserve: candidate eligibility is unchanged')
            expect(page.get_by_text('Still 30.0 GiB short', exact=True)).to_be_visible()
            if evidence:
                evidence.mkdir(parents=True, exist_ok=True)
                page.screenshot(path=str(evidence / 'reserve-80-restarted-wide.png'), full_page=True)
            page.get_by_role('button', name=re.compile('Show saved scenario')).first.click()
            expect(page.get_by_role('heading', name='Placement plan', exact=True)).to_be_visible()
            save_reserve(page, '60')
            expect(page.get_by_role('heading', name='Placement plan', exact=True)).to_have_count(0)
            expect(page.locator('section[aria-labelledby="h-saved"] .tag').filter(has_text='Stale')).to_have_count(3)
            page.get_by_role('button', name=re.compile('Show saved scenario')).first.click()
            expect(page.get_by_role('note').filter(has_text='Historical scenario')).to_be_visible()
            old = call(page, token, 'proposals.get', {'proposal_id': blocked['proposal_id']})['result']
            check(old['summary']['status'] == 'stale' and old['plan'] == blocked, 'reserve: old plan is stale and retains its historical result')
            permitted = simulate(page)
            check(permitted['satisfied'] and len(permitted['proposals']) == 1 and permitted['shortfall_bytes'] == '0', 'reserve: revised 60 GiB reserve permits the same 30 GiB candidate')
            check(permitted['proposal_id'] != blocked['proposal_id'] and permitted['search']['eligible_groups'] == blocked['search']['eligible_groups'], 'reserve: revised input creates a distinct plan with identical eligibility')
            check(not permitted['filesystem_changed'] and all(not p['executable'] for p in permitted['proposals']), 'reserve: placement remains simulation only')
            expect(page.get_by_text('Target met in the simulation', exact=True)).to_be_visible()
            if evidence:
                page.screenshot(path=str(evidence / 'reserve-60-wide.png'), full_page=True)
                page.set_viewport_size({'width': 390, 'height': 844})
                page.get_by_label('Keep at least free for Reserve target', exact=True).scroll_into_view_if_needed()
                expect(page.get_by_role('button', name='Save reserve for Reserve target', exact=True)).to_be_visible()
                check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), 'reserve: narrow viewport has no page overflow')
                page.screenshot(path=str(evidence / 'reserve-60-narrow.png'), full_page=True)
                (evidence / 'reserve-native-plans.json').write_text(json.dumps({'blocked80': blocked, 'historical': old, 'permitted60': permitted}, indent=2) + '\n', encoding='utf-8')
            check(grants() == original_grants and (original.read_bytes(), original.stat().st_mtime_ns) == before, 'reserve: original bytes, modification time and grants are unchanged')
            check(not errors, f'reserve: no browser errors: {errors}')
        finally:
            stop()
