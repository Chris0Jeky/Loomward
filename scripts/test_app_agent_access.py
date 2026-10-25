"""E6 fixture checks for owner client grants and external-volume disclosure."""
from pathlib import Path

from playwright.sync_api import Page, expect


def run_agent_access(page: Page, base: str, shots: Path | None = None) -> None:
    page.goto(f'{base}/?transport=mock#/tiers')
    expect(page.get_by_role('heading', name='Tiers', level=1)).to_be_visible()
    usb = page.locator('article.vol').filter(has=page.get_by_role('heading', name='F:', exact=True))
    expect(usb).to_contain_text('External (USB), may be detached')
    offline = page.locator('article.vol').filter(has=page.get_by_role('heading', name='synthetic-offline', exact=True))
    expect(offline).to_contain_text('offline')
    expect(offline).to_contain_text('External (USB), may be detached')
    unknown = page.locator('article.vol').filter(has=page.get_by_role('heading', name='synthetic-capacity_unknown', exact=True))
    expect(unknown).not_to_contain_text('External')
    print('PASS USB marker, offline USB retained, unknown bus not guessed')

    page.get_by_role('navigation', name='Views').get_by_role('link', name='Agent access', exact=True).click()
    expect(page.get_by_role('heading', name='Agent access', level=1)).to_be_visible()
    table = page.get_by_role('table', name='MCP client grants')
    expect(table).to_contain_text('mcp:synthetic-reader')
    expect(table).to_contain_text('mcp:synthetic-retired')
    expect(table).to_contain_text('Revoked')
    page.get_by_label('Client name', exact=True).fill('synthetic-new')
    folders = page.get_by_role('group', name='Folders', exact=True)
    folders.get_by_role('checkbox').first.check()
    page.get_by_role('checkbox', name='Names and paths', exact=True).check()
    page.get_by_role('checkbox', name='Sizes and the space tools', exact=True).check()
    expect(page.get_by_text("The agent's host may send what it reads to its model provider. Loomward cannot see or control that.", exact=True)).to_be_visible()
    expect(page.locator('pre.connect')).to_contain_text('claude mcp add loomward -- "%LOCALAPPDATA%\\Loomward\\loomward-mcp.exe" --client synthetic-new')
    page.get_by_role('button', name='Create grant', exact=True).click()
    created = table.locator('tbody tr').filter(has_text='mcp:synthetic-new')
    expect(created).to_contain_text('Names and paths')
    expect(created).to_contain_text('Not revoked')
    page.get_by_role('button', name='Revoke mcp:synthetic-new', exact=True).click()
    page.get_by_role('button', name='Keep access', exact=True).click()
    expect(created).to_contain_text('Not revoked')
    page.get_by_role('button', name='Revoke mcp:synthetic-new', exact=True).click()
    page.get_by_role('button', name='Confirm revoke', exact=True).click()
    expect(created.get_by_text('Revoked', exact=True)).to_be_visible()
    expect(page.get_by_role('status').filter(has_text='Access revoked.')).to_be_visible()
    if shots:
        shots.mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(shots / 'agent-access-desktop.png'), full_page=True)
    page.set_viewport_size({'width': 375, 'height': 800})
    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'), 'Agent access reflows at 375 px'
    scroller = page.get_by_role('region', name='Scrollable MCP client grants')
    scroller.focus()
    scroller.press('ArrowRight')
    page.wait_for_function("document.querySelector('[aria-label=\"Scrollable MCP client grants\"]').scrollLeft > 0")
    scroller.evaluate('(element) => { element.scrollLeft = 0; }')
    if shots:
        page.screenshot(path=str(shots / 'agent-access-mobile.png'), full_page=True)
    page.set_viewport_size({'width': 1360, 'height': 900})
    print('PASS Agent access: contract history, create, disclosure, revoke confirmation and cancellation')
