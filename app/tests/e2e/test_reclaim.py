"""LW-131 R1d: facts only, honest unknowns and no execute affordance."""
from copy import deepcopy
import json
from pathlib import Path

from playwright.sync_api import expect


def run(ctx) -> dict:
    page, base, check = ctx.page, ctx.base, ctx.check
    headings = ['Tool-owned cleanup', 'Relocate', 'Archive candidates', 'Informational']

    def read_only():
        main = page.locator('main')
        expect(main.get_by_role('button')).to_have_text(['Look again', 'Observe hard links', 'Observe selection', 'Clear selection', 'Record before', 'Check now'])
        expect(main.get_by_role('combobox')).to_have_count(1)
        # #1069: the only links are plain navigation to Explorer's inspector (a hash route, no effect); still no form or button-role.
        expect(main.locator('form, [role="button"]')).to_have_count(0)
        expect(main.locator('a:not([href^="#/explorer?node="])')).to_have_count(0)
        expect(main.locator('input:not([type="checkbox"]):not([inputmode="numeric"])')).to_have_count(0)

    page.goto(f'{base}/?transport=mock#/reclaim')
    expect(page.get_by_role('heading', name='Reclaim', level=1)).to_be_visible()
    expect(page.get_by_label('Volume to review')).to_have_value('vo_f')
    for heading in headings:
        expect(page.get_by_role('heading', name=heading, exact=True, level=2)).to_be_visible()
    expect(page.locator('main h2')).to_have_text(['Volume', 'Inspect a selection', 'Relief check', *headings])
    cleanup = page.get_by_role('region', name='Tool-owned cleanup', exact=True)
    expect(cleanup).to_contain_text('target')
    expect(cleanup).to_contain_text('Allocated: 7.81 KiB')
    expect(cleanup).to_contain_text('Estimated reclaimable: unknown')
    expect(cleanup).to_contain_text('hard links outside the candidate, or not observed')
    expect(cleanup).to_contain_text('Coverage: Complete')
    expect(cleanup).to_contain_text('cargo clean --dry-run')
    expect(cleanup).to_contain_text('CARGO_TARGET_DIR')
    expect(cleanup.get_by_role('list', name='Evidence')).to_contain_text('Cargo.toml')
    expect(cleanup.get_by_role('link', name='Inspect in Explorer: target')).to_have_attribute('href', '#/explorer?node=nd_synthetic')
    expect(cleanup).to_contain_text('Uncertainty: Names only')
    expect(page.get_by_label('Unaccounted space')).to_contain_text('Unaccounted: 68.3 KiB')
    read_only()
    # R1g: the observe job is metadata-only and changes nothing; it turns one unknown relief into a number.
    expect(page.get_by_text('changes nothing.', exact=False)).to_be_visible()
    page.get_by_role('button', name='Observe hard links', exact=True).click()
    status = page.get_by_label('Observation status')
    expect(status).to_contain_text('Observing hard links')
    expect(page.get_by_role('button', name='Cancel observation', exact=True)).to_be_visible()
    expect(status).to_contain_text('candidates observed')
    expect(cleanup).to_contain_text('Estimated reclaimable: 6.00 KiB', timeout=10000)
    expect(status).to_contain_text('Observation finished')
    expect(cleanup).not_to_contain_text('hard links outside the candidate, or not observed')
    expect(page.get_by_role('button', name='Cancel observation')).to_have_count(0)
    read_only()
    choice = page.get_by_role('checkbox', name='Include target in selection', exact=True)
    target = page.get_by_label('Space target (bytes)', exact=True)
    estimate = page.get_by_label('Selection estimate', exact=True)
    target.fill('6145')
    choice.focus()
    page.keyboard.press('Space')
    expect(choice).to_be_checked()
    page.get_by_role('button', name='Observe selection', exact=True).click()
    # Changing the selection while the request runs cannot adopt its eventual result.
    choice.uncheck()
    expect(estimate).to_contain_text('Union estimate: unknown')
    expect(page.get_by_role('button', name='Cancel observation')).to_have_count(0, timeout=10000)
    expect(estimate).not_to_contain_text('6144 bytes')
    choice.check()
    page.get_by_role('button', name='Observe selection', exact=True).click()
    expect(estimate).to_contain_text('6144 bytes', timeout=10000)
    expect(estimate).to_contain_text('1 bytes short')
    expect(estimate).to_contain_text('Recorded less than a minute ago')
    target.fill('9007199254740993')
    expect(estimate).to_contain_text('9007199254734849 bytes short')
    target.fill('1e3')
    expect(target).to_have_attribute('aria-invalid', 'true')
    expect(page.get_by_role('button', name='Observe selection', exact=True)).to_be_disabled()
    target.fill('0')
    page.get_by_role('button', name='Observe selection', exact=True).click()
    page.get_by_role('button', name='Cancel observation', exact=True).click()
    expect(status).to_contain_text('Observation cancelled', timeout=10000)
    expect(estimate).to_contain_text('Union estimate: unknown')
    expect(estimate).not_to_contain_text('6144 bytes')
    page.get_by_label('Volume to review').select_option(index=1)
    expect(page.get_by_role('button', name='Look again', exact=True)).to_be_enabled()
    expect(cleanup).to_contain_text('Estimated reclaimable: unknown')
    expect(choice).not_to_be_checked()
    for width in [1440, 390]:
        page.set_viewport_size({'width': width, 'height': 900})
        check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'Reclaim fits at {width}px')
        ctx.view_shot(f'reclaim-{width}.png')

    # A controlled HTTP reply exercises states not present in the fixed contract example.
    root = Path(__file__).resolve().parents[3]
    examples = root / 'contracts/v3/examples/commands'
    report = json.loads((examples / 'reclaim.plan.result.json').read_text(encoding='utf-8'))
    volume_list = json.loads((examples / 'volumes.list.result.json').read_text(encoding='utf-8'))
    report.update(partial=True, truncated=True, budget_hit=True, unaccounted_bytes=None)
    candidate = report['groups'][0]['candidates'][0]
    candidate.update(name='<img src=x onerror=alert(1)>', logical_bytes=None, allocated_bytes=None,
                     owner_step='<a href="https://example.invalid">cargo clean</a>', coverage='partial')
    # A candidate with no catalogue node (the Recycle Bin pseudo candidate) gets no Explorer link.
    report['groups'][0]['candidates'].append({**deepcopy(candidate), 'node_id': None, 'name': 'Recycle Bin'})
    inspected = []
    failed = False
    selection_status = 'recorded'
    requests = []

    def reply(route):
        request = route.request.post_data_json
        if request['command'] == 'node.inspect':
            inspected.append(request)
            detail = json.loads((examples / 'node.inspect.result.json').read_text(encoding='utf-8'))
            route.fulfill(json={'protocol': 'loomward/3', 'request_id': request['request_id'], 'ok': True, 'result': detail,
                                'meta': {'served_at': '2026-10-01T00:00:00Z', 'elapsed_ms': 0, 'dataset_class': 'synthetic',
                                         'budget_hit': False, 'catalog_rev': '1', 'state_rev': '1'}})
            return
        if request['command'] not in ('volumes.list', 'reclaim.plan'):
            route.continue_()
            return
        if request['command'] == 'reclaim.plan':
            requests.append(request['payload'])
        if failed and request['command'] == 'reclaim.plan':
            route.fulfill(json={'protocol': 'loomward/3', 'request_id': request['request_id'], 'ok': False,
                                'error': {'code': 'not_found', 'message': 'unknown volume', 'retryable': False, 'detail': None}})
            return
        result = deepcopy(volume_list if request['command'] == 'volumes.list' else report)
        if request['command'] == 'reclaim.plan':
            result['volume']['volume_id'] = request['payload']['volume_id']
            if 'selection' in request['payload']:
                result['selection'] = {'members': request['payload']['selection'], 'status': selection_status,
                    'observed_at': '2026-10-01T00:00:00Z',
                    'reclaimable_bytes': '9007199254740993' if selection_status == 'recorded' else None,
                    'reason': None if selection_status == 'recorded' else 'A selected member changed. Observe again.'}
        route.fulfill(json={'protocol': 'loomward/3', 'request_id': request['request_id'], 'ok': True, 'result': result,
                            'meta': {'served_at': '2026-10-01T00:00:00Z', 'elapsed_ms': 0, 'dataset_class': 'synthetic',
                                     'budget_hit': False, 'catalog_rev': '1', 'state_rev': '1'}})

    page.route('**/api/v3/call', reply)
    try:
        page.goto(f'{base}/#token=a1b2c3d4e5f60718293a4b5c6d7e8f90')
        page.evaluate("location.hash = '#/reclaim'")
        expect(cleanup).to_contain_text('Allocated: unknown')
        expect(cleanup).to_contain_text('Logical: unknown')
        expect(cleanup).to_contain_text('Estimated reclaimable: unknown')
        expect(cleanup).not_to_contain_text('0 B')
        expect(cleanup).to_contain_text(candidate['name'])
        expect(cleanup).to_contain_text(candidate['owner_step'])
        expect(cleanup.locator('img')).to_have_count(0)
        # Only the node-bearing candidate links out; the hostile owner step stays text and the Recycle Bin pseudo candidate has no link.
        expect(cleanup.locator('a')).to_have_count(1)
        expect(cleanup.get_by_role('listitem').filter(has_text='Recycle Bin').get_by_role('link')).to_have_count(0)
        expect(page.get_by_text('The search ran out of budget.', exact=True)).to_be_visible()
        expect(page.get_by_text('This volume is not fully known.', exact=True)).to_be_visible()
        expect(page.get_by_text('More candidates exist than are shown.', exact=False)).to_be_visible()
        expect(page.get_by_label('Unaccounted space')).to_contain_text('Unaccounted: unknown — coverage or volume measurements are partial or unknown.')
        read_only()
        check(requests[-1] == {'volume_id': page.get_by_label('Volume to review').input_value(), 'limit': 50},
              'Reclaim sends exactly the listed volume identity and limit 50')
        # Following the link opens Explorer on that node alone: the plan has no generation, so none is sent.
        cleanup.get_by_role('link', name='Inspect in Explorer', exact=False).click()
        inspector = page.get_by_role('region', name='Node inspector')
        expect(inspector).to_contain_text('report_final.docx', timeout=10000)
        expect(inspector).to_contain_text(r'Location: Lab\S1\Projects\Alpha\report_final.docx')
        check(inspected and inspected[-1]['payload'] == {'node_id': 'nd_synthetic'} and 'expected_generation' not in inspected[-1],
              'Explorer inspects the linked node without an expected generation')
        page.go_back()
        expect(cleanup).to_contain_text(candidate['name'])
        page.get_by_role('checkbox').check()
        target.fill('9007199254740992')
        page.get_by_role('button', name='Look again', exact=True).click()
        expect(estimate).to_contain_text('9007199254740993 bytes')
        expect(estimate).to_contain_text('1 bytes above')
        selection_status = 'expired'
        page.get_by_role('button', name='Look again', exact=True).click()
        expect(estimate).to_contain_text('This observation has expired.')
        expect(estimate).to_contain_text('Target coverage is unknown.')
        expect(estimate).not_to_contain_text('9007199254740993 bytes')
        check(set(requests[-1]) == {'volume_id', 'limit', 'selection'}, 'Selection sends IDs, never paths or an execution instruction')
        failed = True
        page.get_by_role('button', name='Look again', exact=True).click()
        expect(page.locator('main [role="alert"]')).to_have_text('unknown volume')
        expect(cleanup).to_have_count(0)
        expect(page.get_by_label('Unaccounted space')).to_have_count(0)
    finally:
        page.unroute('**/api/v3/call', reply)
        page.set_viewport_size({'width': 1360, 'height': 900})
    check(True, 'Reclaim: four groups, unknown sizes and reasons, unaccounted, partial/budget/truncated, text-only owner steps, no action')
    return {'groups': headings, 'read_only': True, 'unknowns': True}
