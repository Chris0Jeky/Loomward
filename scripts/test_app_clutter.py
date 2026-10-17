"""LW-119 developer clutter: the mock transport's scripted report (candidates, an unknown size, a budget hit,
a root with no evidence) and the empty state against the real service. Advice only: no control acts on a folder.
Synthetic names only."""
from __future__ import annotations
import re
from playwright.sync_api import Page, expect


def run_clutter(page: Page, base: str) -> None:
    page.goto(f'{base}/?transport=mock#/clutter')
    expect(page.get_by_role('heading', name='Developer clutter', level=1)).to_be_visible()
    expect(page.get_by_text('You could remove these; Loomward will not.')).to_be_visible()
    table = page.get_by_role('table', name=re.compile('^Candidate folders'))
    expect(table).to_be_visible()
    rows = table.locator('tbody tr')
    expect(rows).to_have_count(2)
    # Ranked by recorded size, each beside the manifest that made its name a candidate.
    expect(rows.nth(0)).to_contain_text('target')
    expect(rows.nth(0)).to_contain_text('beside Cargo.toml')
    expect(rows.nth(0)).to_contain_text('2.00 GiB')
    expect(rows.nth(0)).to_contain_text('Build output')
    # An unknown size stays the word "unknown", never 0.
    expect(rows.nth(1)).to_contain_text('node_modules')
    expect(rows.nth(1).locator('td.r')).to_contain_text('unknown')
    assert '0 B' not in rows.nth(1).inner_text(), 'an unknown size must not read as zero'
    expect(rows.nth(1)).to_contain_text('Rebuild cost: not assessed yet.')
    # Rebuild cost, model hosts and stale worktrees are not assessed, and say so.
    items = page.get_by_role('region', name='Not assessed yet').get_by_role('listitem')
    expect(items).to_have_count(3)
    for i in range(3):
        expect(items.nth(i)).to_contain_text('Not assessed yet')
    # Advice only: the one button reads again, nothing acts on a folder.
    buttons = page.locator('main').get_by_role('button')
    expect(buttons).to_have_count(1)
    expect(buttons.first).to_have_text('Look again')
    # The root with no evidence is never reported clean.
    select = page.get_by_label('Granted root')
    options = select.locator('option')
    assert options.count() >= 2, 'the mock offers more than one active root'
    select.select_option(index=1)
    expect(page.get_by_text('None found in what has been scanned. That is not a clean result.')).to_be_visible()
    expect(page.get_by_text('This root is not fully known.')).to_be_visible()
    expect(table).to_have_count(0)
    # A search that ran out of budget says so and lists nothing.
    select.select_option(value='rt_mock_9')
    expect(page.get_by_text('The search ran out of budget.')).to_be_visible()
    expect(table).to_have_count(0)
    expect(page.get_by_text('No build output or cache folder is recorded')).to_have_count(0)
    # A phone-width window scrolls the table, not the page.
    page.set_viewport_size({'width': 390, 'height': 844})
    select.select_option(index=0)
    expect(table).to_be_visible()
    assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'the page scrolls sideways at 390px'
    page.set_viewport_size({'width': 1440, 'height': 900})
    print('PASS developer clutter (mock): ranked advice, unknown size, not-assessed, partial and budget-hit states, no action')


def live_clutter(page: Page, token: str, check, live_call) -> None:
    """Over the real HTTP adapter, with no granted root: an unknown root is not_found, and the view lists no root rather than demo data."""
    unknown = live_call(page, token, 'clutter.developer', {'root_id': 'rt_unknownroot', 'limit': 5})
    check(not unknown['ok'] and unknown['error']['code'] == 'not_found', f'clutter.developer refuses an unknown root: {unknown}')
    bad = live_call(page, token, 'clutter.developer', {'root_id': 'rt_unknownroot', 'limit': 201})
    check(not bad['ok'] and bad['error']['code'] == 'invalid_request', f'an out-of-range limit is refused: {bad}')
    page.evaluate("location.hash = '#/clutter'")
    expect(page.get_by_role('heading', name='Developer clutter', level=1)).to_be_visible()
    expect(page.get_by_text('No granted root is available. Add and scan a folder first.')).to_be_visible()
    check(page.locator('main p.bad.live').inner_text() == '', 'clutter: the view loaded from the engine with no error')
    print('PASS live developer clutter: unknown root refused, no demo roots')
