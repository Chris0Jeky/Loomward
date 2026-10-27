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
        # The only controls: read again, and the read-only observe job (cancel appears only while it runs).
        expect(main.get_by_role('button')).to_have_text(['Look again', 'Observe hard links'])
        expect(main.get_by_role('combobox')).to_have_count(1)
        expect(main.locator('a, form, input, [role="button"]')).to_have_count(0)

    page.goto(f'{base}/?transport=mock#/reclaim')
    expect(page.get_by_role('heading', name='Reclaim', level=1)).to_be_visible()
    expect(page.get_by_label('Volume to review')).to_have_value('vo_f')
    for heading in headings:
        expect(page.get_by_role('heading', name=heading, exact=True, level=2)).to_be_visible()
    expect(page.locator('main h2')).to_have_text(['Volume', *headings])
    cleanup = page.get_by_role('region', name='Tool-owned cleanup', exact=True)
    expect(cleanup).to_contain_text('target')
    expect(cleanup).to_contain_text('Allocated: 7.81 KiB')
    expect(cleanup).to_contain_text('Estimated reclaimable: unknown')
    expect(cleanup).to_contain_text('hard links outside the candidate, or not observed')
    expect(cleanup).to_contain_text('Coverage: Complete')
    expect(cleanup).to_contain_text('cargo clean --dry-run')
    expect(cleanup).to_contain_text('CARGO_TARGET_DIR')
    expect(cleanup.get_by_role('list', name='Evidence')).to_contain_text('Cargo.toml')
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
    page.get_by_label('Volume to review').select_option(index=1)
    expect(page.get_by_role('button', name='Look again', exact=True)).to_be_enabled()
    expect(cleanup).to_contain_text('Estimated reclaimable: unknown')
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
    failed = False
    requests = []

    def reply(route):
        request = route.request.post_data_json
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
        expect(cleanup.locator('img, a')).to_have_count(0)
        expect(page.get_by_text('The search ran out of budget.', exact=True)).to_be_visible()
        expect(page.get_by_text('This volume is not fully known.', exact=True)).to_be_visible()
        expect(page.get_by_text('More candidates exist than are shown.', exact=False)).to_be_visible()
        expect(page.get_by_label('Unaccounted space')).to_contain_text('Unaccounted: unknown — coverage or volume measurements are partial or unknown.')
        read_only()
        check(requests[-1] == {'volume_id': page.get_by_label('Volume to review').input_value(), 'limit': 50},
              'Reclaim sends exactly the listed volume identity and limit 50')
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
