"""Synthetic Applications/Desktop checks, reused by mock, accessibility and live-serve legs."""
import re

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
