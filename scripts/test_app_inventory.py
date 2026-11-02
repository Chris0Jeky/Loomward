"""Synthetic Applications/Desktop checks, reused by mock, accessibility and live-serve legs."""
import re
from copy import deepcopy

from playwright.sync_api import expect


def run_inventory(page, base, check, *, live=False, shots=None):
    if not live:
        page.goto(f'{base}/?transport=mock#/explorer')
    for route, title in [('applications', 'Applications'), ('desktop', 'Desktop')]:
        page.get_by_role('link', name=title, exact=True).click()
        expect(page.get_by_role('heading', name=title, level=1)).to_be_focused()
        expect(page.locator('main table tbody tr').first).to_be_visible()
        expect(page.locator('main [role=status]')).to_contain_text('page 1 loaded')
        expect(page.get_by_role('button', name='Next page', exact=True)).to_be_disabled()
        if route == 'applications':
            expect(page.locator('main table tbody tr')).to_have_count(2)
            expect(page.locator('main table tbody tr').nth(1)).to_contain_text('unknown')
            row = page.locator('main table tbody tr').nth(1)
            expect(row.locator('td').nth(0)).to_contain_text('synthetic publisher unreadable')
            expect(row.locator('td').nth(0)).to_contain_text('synthetic version malformed')
            expect(row.locator('td').nth(3)).to_contain_text('synthetic location unavailable')
            expect(page.locator('main')).to_contain_text('denied')
            expect(page.locator('main')).to_contain_text('packaged')
            expect(page.locator('main')).to_contain_text('unavailable')
        else:
            expect(page.locator('main table tbody tr')).to_have_count(4)
            health = lambda row: page.locator('main table tbody tr').nth(row).locator('td').first
            expect(health(0)).to_contain_text('missing')
            expect(health(1)).to_contain_text('volume unavailable')
            expect(health(2)).to_contain_text('network unreachable unknown')
            expect(page.locator('main dl.contexts')).to_contain_text('redirected')
            expect(page.locator('main dl.contexts')).to_contain_text('public')
        expect(page.locator('main table')).to_contain_text('<img src=x onerror=alert(1)>')
        check(page.locator('main img').count() == 0, f'{title}: hostile evidence stays text')
        controls = page.locator('main button').all_text_contents()
        check(not any(re.search(r'launch|uninstall|repair|delete|execute|fix|apply', x, re.I) for x in controls), f'{title}: read-only controls')
        status = page.locator('main [role=status]').element_handle()
        page.get_by_role('button', name=f'Refresh {route if route == "applications" else "desktop"}').click()
        expect(page.locator('main table tbody tr').first).to_be_visible()
        expect(page.locator('main [role=status]')).to_contain_text('page 1 loaded')
        check(status.evaluate('e => e.isConnected'), f'{title}: refresh uses the persistent live region')
        for theme in ['Woven atlas', 'Observatory']:
            page.get_by_role('button', name=theme, exact=True).click()
            for width, height in [(1440, 900), (390, 844)]:
                page.set_viewport_size({'width': width, 'height': height})
                check(page.evaluate('document.documentElement.scrollWidth <= innerWidth + 1'), f'{title}: {theme} reflows at {width}px')
                table = page.get_by_role('region', name='Installed applications' if route == 'applications' else 'Desktop shortcuts')
                table.focus()
                expect(table).to_be_focused()
                check(table.get_attribute('tabindex') == '0', f'{title}: table scrolling is keyboard accessible')
                if shots:
                    shots.mkdir(parents=True, exist_ok=True)
                    page.screenshot(path=str(shots / f'{route}-{theme.replace(" ", "-")}-{width}.png'), full_page=True)
        page.set_viewport_size({'width': 1440, 'height': 900})
    check(True, 'Applications and Desktop through the synthetic live service' if live else 'Applications and Desktop through the synthetic mock')
    if live:
        def refuse(route):
            req = route.request.post_data_json
            if req['command'] == 'apps.list':
                route.fulfill(json={'protocol': 'loomward/3', 'request_id': req['request_id'], 'ok': False,
                                    'error': {'code': 'resource_budget', 'message': 'Synthetic inventory refused', 'retryable': True, 'detail': None}})
            else:
                route.continue_()
        page.route('**/api/v3/call', refuse)
        page.get_by_role('link', name='Applications', exact=True).click()
        expect(page.locator('main p.bad')).to_contain_text('Synthetic inventory refused')
        check(page.locator('main table').count() == 0, 'Applications: a refused refresh removes earlier data')
        page.unroute('**/api/v3/call', refuse)
        page.get_by_role('button', name='Refresh applications').click()
        expect(page.locator('main table tbody tr')).to_have_count(2)
        expect(page.locator('main p.bad')).to_have_count(0)
        check(True, 'Applications: recovery uses real synthetic service data')
        run_inventory_sort(page, check)


def run_inventory_sort(page, check):
    """Exercise the rendered toggle and paging with routed synthetic inventory, not host apps."""
    requests = []
    template = None
    # More than the UI's 200-row page: registry order starts small/unknown and the largest
    # estimates are on page two. Sorting only the visible page cannot satisfy these assertions.
    names = [f'Synthetic sort app {i:03}' for i in range(203)]
    size_order = list(range(202, 1, -1)) + [0, 1]

    def inventory(route):
        nonlocal template
        req = route.request.post_data_json
        if req['command'] != 'apps.list':
            route.continue_()
            return
        payload = req['payload']
        requests.append(payload)
        assert payload['limit'] == 200
        order = payload['sort']
        assert order in ('registry', 'size_desc')
        cursor = payload['cursor']
        assert cursor in (None, f'synthetic_{order}_200'), 'cursor must belong to the requested sort'
        if template is None:
            assert cursor is None
            template = route.fetch().json()
            assert template['ok'], 'synthetic service inventory must be available'
        response = deepcopy(template)
        response['request_id'] = req['request_id']
        indices = list(range(203)) if order == 'registry' else size_order
        offset = 0 if cursor is None else 200
        rows = []
        for i in indices[offset:offset + 200]:
            row = deepcopy(template['result']['applications'][0])
            row['registry_key'] = f'SyntheticSort{i:03}'
            row['name']['value'] = names[i]
            row['estimated_size_bytes'] = None if i == 1 else str(i + 1)
            row['size_reason'] = 'synthetic unknown estimate' if i == 1 else None
            rows.append(row)
        response['result']['applications'] = rows
        response['result']['next_cursor'] = f'synthetic_{order}_200' if offset == 0 else None
        route.fulfill(json=response)

    page.route('**/api/v3/call', inventory)
    try:
        refresh = page.get_by_role('button', name='Refresh applications', exact=True)
        next_page = page.get_by_role('button', name='Next page', exact=True)
        largest = page.get_by_role('checkbox', name=re.compile('^Largest first'))
        status = page.locator('main [role=status]')
        rows = page.locator('main table tbody tr')
        headings = rows.locator('th[scope=row]')
        expect(largest).not_to_be_checked()
        refresh.click()
        expect(headings).to_have_text(names[:200])
        expect(status).to_contain_text('page 1 loaded')
        expect(rows.nth(1)).to_contain_text('synthetic unknown estimate')
        next_page.click()
        expect(headings).to_have_text(names[200:])
        expect(status).to_contain_text('page 2 loaded')
        expect(next_page).to_be_disabled()

        largest.click()
        expect(largest).to_be_checked()
        expect(largest).to_be_focused()
        expect(headings).to_have_text([names[i] for i in size_order[:200]])
        expect(page.locator('main')).to_contain_text('Sizes are provider estimates, not disk measurements.')
        expect(status).to_contain_text('page 1 loaded')
        expect(largest).to_be_focused()
        next_page.click()
        expect(headings).to_have_text([names[i] for i in size_order[200:]])
        expect(status).to_contain_text('page 2 loaded')
        expect(rows.last).to_contain_text('synthetic unknown estimate')
        expect(next_page).to_be_disabled()

        largest.click()
        expect(largest).not_to_be_checked()
        expect(headings).to_have_text(names[:200])
        expect(status).to_contain_text('page 1 loaded')
        expect(largest).to_be_focused()
        check([(r['sort'], r['cursor']) for r in requests] == [
            ('registry', None), ('registry', 'synthetic_registry_200'),
            ('size_desc', None), ('size_desc', 'synthetic_size_desc_200'), ('registry', None),
        ], 'Applications: Largest first changes rendered order, resets paging in both directions, and keeps unknown estimates last (203 routed synthetic apps)')
    finally:
        page.unroute('**/api/v3/call', inventory)
