"""C7 browser proof: the real mock gate plus a scripted synthetic engine, never a copier."""
from __future__ import annotations

import json
from pathlib import Path

from playwright.sync_api import Page, expect
from test_app_views import FakeEngine, TOKEN, ok, wait_until
from test_app_a11y import duplicate_names

DIGEST = 'sha256:' + 'a' * 64
HOSTILE = '<copy>\u202e.txt'

RECEIPTS = Path(__file__).resolve().parents[1] / 'contracts/v3/examples/dtos'
READ_BACK = json.loads((RECEIPTS / 'copy-receipt.json').read_text())['value']
PUBLISHED = json.loads((RECEIPTS / 'copy-receipt-published-unverified.json').read_text())['value']


class CopyEngine(FakeEngine):
    """Only test responses: no content reads, destination writes or runtime mock bypass."""

    def __init__(self):
        super().__init__()
        self.calls = []
        self.state = 'draft'
        self.refused = True
        self.personal = False

    def result(self, req):
        command = req['command']
        if command == 'tree.slice':
            return {'nodes': [{'node_id': 'nd_atlas', 'kind': 'atlas', 'name': 'Atlas', 'parent': None},
                              {'node_id': 'nd_parent', 'kind': 'root', 'name': 'Synthetic parent', 'parent': 0}]}
        if command == 'session.hello':
            result = super().result(req)
            result['dataset_class'] = 'personal' if self.personal else 'synthetic'
            return result
        if command == 'tree.children':
            result = super().result(req)
            result['items'] = [{**result['items'][0], 'name': HOSTILE}]
            result['next_cursor'] = None
            return result
        if not command.startswith('copy.'):
            return super().result(req)
        self.calls.append(req)
        if command == 'copy.preview':
            self.state = 'refused' if self.refused else 'draft'
            return {'operation_id': 'cp_scripted', 'state': self.state, 'digest': DIGEST,
                    'folder_name': '<new>\u200b', 'files': 1, 'folders': 0, 'bytes': '1000',
                    'space_needed': '4096', 'space_reserve': '1073741824', 'same_volume': True,
                    'explicit_acl_items': 1, 'hard_link_names': 0,
                    'refusal_count': 2 if self.refused else 0,
                    'refusals': [{'code': code, 'node_id': 'nd_item-000.txt'} for code in ('cloud_placeholder', 'extended_attributes')] if self.refused else []}
        if command == 'copy.approve':
            return {'operation_id': 'cp_scripted', 'digest': DIGEST,
                    'approved_at': '2026-10-17T00:00:00Z', 'expires_at': '2099-10-17T00:30:00Z'}
        if command == 'copy.start':
            self.state = 'copying'
        if command in ('copy.cancel', 'copy.abandon'):
            self.state = 'cancelled'
        return {'operation_id': 'cp_scripted', 'digest': DIGEST, 'state': self.state,
                'items_total': 3, 'items_verified': 3 if self.state == 'committed' else 0,
                **({'receipt': READ_BACK} if self.state == 'committed' else
                   {'receipt': PUBLISHED} if self.state == 'publishing' else {}),
                'refusal': {'code': 'source_in_use', 'node_id': 'nd_item-000.txt'} if self.state == 'blocked' else None}


def select_parent(page: Page):
    page.get_by_role('button', name='Browse destination root Synthetic parent').click()
    page.get_by_role('button', name='Use this folder as parent').click()


def run_copy(page: Page, base: str, shots: Path | None = None):
    page.goto(f'{base}/?transport=mock#/explorer')
    page.get_by_role('checkbox', name='Select', exact=False).first.check()
    page.get_by_role('button', name='Copy to…', exact=True).click()
    expect(page.get_by_role('heading', name='Copy & verify', level=1)).to_be_focused()
    expect(page.get_by_role('heading', name='Copying is not enabled yet', exact=True)).to_be_visible()
    page.get_by_role('button', name='Browse destination root', exact=False).first.click()
    page.get_by_role('button', name='Use this folder as parent').click()
    page.get_by_role('button', name='Build preview').click()
    expect(page.get_by_role('alert')).to_contain_text('copy_effect_gated')
    expect(page.get_by_role('button', name='Approve this preview')).to_have_count(0)
    ok(True, 'copy: real mock refuses preview visibly, with no invented manifest or approval')

    engine = CopyEngine()
    engine.start()
    try:
        page.goto(f'{engine.base}/#token={TOKEN}')
        page.evaluate("location.hash = '#/explorer'")
        page.get_by_role('checkbox', name='Select', exact=False).first.check()
        page.get_by_role('button', name='Copy to…', exact=True).click()
        select_parent(page)
        page.get_by_role('button', name='Build preview').click()
        operation = page.get_by_role('region', name='Copy operation')
        expect(operation).to_contain_text('cloud_placeholder')
        expect(operation).to_contain_text('extended_attributes')
        expect(operation).to_contain_text('cannot yet be preserved')
        expect(page.get_by_role('button', name='Approve this preview')).to_have_count(0)
        ok(True, 'copy: every manifest refusal has its code and plain-language reason; approval stays blocked')
        engine.refused = False
        page.get_by_role('button', name='Build preview').click()
        page.get_by_role('button', name='Approve this preview').click()
        expect(page.get_by_role('button', name='Start copying')).to_be_enabled()
        ok(engine.calls[-1]['payload'] == {'operation_id': 'cp_scripted', 'digest': DIGEST}, 'copy: explicit approval binds the preview digest')
        page.get_by_role('button', name='Use this folder as parent').click()
        expect(page.get_by_role('status').filter(has_text='Approval invalidated')).to_be_visible()
        expect(page.get_by_role('button', name='Start copying')).to_have_count(0)
        page.get_by_role('button', name='Build preview').click()
        page.get_by_role('button', name='Approve this preview').click()
        page.get_by_role('button', name='Start copying').focus()
        page.keyboard.press('Enter')
        expect(page.get_by_role('heading', name='copying', exact=True)).to_be_focused()
        refresh = page.get_by_role('button', name='Refresh copy status')
        refresh.focus()
        before = len(engine.calls)
        wait_until(page, lambda: any(r['command'] == 'copy.status' for r in engine.calls[before:]))
        expect(refresh).to_be_focused()
        ok(True, 'copy: read-only progress polling never steals keyboard focus')
        page.get_by_role('button', name='Cancel copy').click()
        expect(page.get_by_role('heading', name='Receipt', exact=True)).to_be_visible()
        ok(engine.calls[-1]['command'] == 'copy.cancel', 'copy: keyboard start and cancellation retain the engine outcome')
        page.get_by_role('button', name='Build preview').click()
        page.get_by_role('button', name='Approve this preview').click()
        page.get_by_role('button', name='Start copying').click()
        expect(page.get_by_role('heading', name='copying', exact=True)).to_be_focused()
        engine.state = 'blocked'
        page.get_by_role('button', name='Refresh copy status').click()
        expect(page.get_by_role('button', name='Confirm resume')).to_be_visible()
        page.get_by_role('button', name='Abandon operation').click()
        expect(page.get_by_role('heading', name='Receipt', exact=True)).to_be_visible()
        page.get_by_role('button', name='Build preview').click()
        page.get_by_role('button', name='Approve this preview').click()
        page.get_by_role('button', name='Start copying').click()
        expect(page.get_by_role('heading', name='copying', exact=True)).to_be_focused()
        engine.state = 'publishing'
        page.get_by_role('button', name='Refresh copy status').click()
        expect(page.get_by_role('button', name='Cancel copy')).to_have_count(0)
        expect(operation).to_contain_text('published_unverified')
        engine.state = 'committed'
        page.get_by_role('button', name='Refresh copy status').click()
        expect(page.get_by_role('heading', name='Receipt', exact=True)).to_be_visible()
        expect(operation).to_contain_text(READ_BACK['explanation'])
        expect(operation).to_contain_text('not power-loss tested on this disk')
        assert 'durable' not in operation.inner_text().lower()
        expect(operation).to_contain_text('No space was freed on the source')
        expect(operation).to_contain_text('unknown / unknown')
        expect(operation).to_contain_text('U+200B')
        expect(page.locator('main copy, main new, main script')).to_have_count(0)
        ok(not duplicate_names(page), 'copy: controls have unique accessible names, hostile names stay text, unknowns stay visible')
        for width in (1440, 390):
            page.set_viewport_size({'width': width, 'height': 900})
            ok(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'copy: no page overflow at {width}px')
            if shots:
                shots.mkdir(parents=True, exist_ok=True)
                page.screenshot(path=str(shots / f'copy-scripted-{width}.png'), full_page=True)
        engine.personal = True
        page.goto(f'{engine.base}/?personal-test=1#token={TOKEN}')
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Dataset personal')
        page.evaluate("location.hash = '#/explorer'")
        page.get_by_role('checkbox', name='Select', exact=False).first.check()
        page.get_by_role('button', name='Copy to…', exact=True).click()
        select_parent(page)
        page.get_by_role('button', name='Build preview').click()
        expect(page.get_by_role('button', name='Approve this preview')).to_be_disabled()
        expect(operation).to_contain_text('Personal approval is unavailable over HTTP')
        ok(True, 'copy: personal HTTP sessions cannot invoke synthetic consent')
    finally:
        engine.stop()
        page.set_viewport_size({'width': 1360, 'height': 900})


if __name__ == '__main__':
    import argparse
    import threading
    import test_app as app
    from playwright.sync_api import sync_playwright

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    app.DIST = Path(__file__).resolve().parents[1] / '.loomward/app-e2e'
    app.build_app(app.DIST, development=True)
    server = app.ThreadingHTTPServer(('127.0.0.1', 0), app.Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            page = browser.new_page(viewport={'width': 1360, 'height': 900})
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.on('console', lambda message: errors.append(message.text) if message.type == 'error' else None)
            run_copy(page, f'http://127.0.0.1:{server.server_port}', args.screenshots)
            ok(not errors, f'copy: no console or page errors: {errors}')
            browser.close()
    finally:
        server.shutdown()
