"""#1098 Today comparison rendering over the real HTTP transport with synthetic answers.

Coordinator hook: import run_new_since from test_app_new_since and call
run_new_since(page.context.browser) alongside the other run_views browser checks.
"""
from copy import deepcopy
import json
import re
from pathlib import Path

from playwright.sync_api import expect
from test_app_views import FakeEngine, TOKEN, wait_until

FIXTURES = Path(__file__).resolve().parents[1] / 'contracts/v3/examples/commands'


def fixture(command):
    return json.loads((FIXTURES / f'{command}.result.json').read_text())


class ComparisonEngine(FakeEngine):
    def __init__(self):
        super().__init__()
        self.roots = fixture('roots.list')['roots'][:1]
        self.mode = 'known'
        self.finished_at = '2026-10-23T00:30:00Z'
        self.inspected = []

    def result(self, req):
        command = req['command']
        if command == 'node.inspect':
            node_id = req['payload']['node_id']
            self.inspected.append(node_id)
            answer = fixture(command)
            answer['row'].update(node_id=node_id, kind='dir', name='New folder', extension=None, ext_family=None)
            return answer
        if command == 'volumes.list':
            return fixture(command)
        if command == 'changes.new_since':
            if self.mode == 'missing':
                return ('resource_budget', 'Synthetic bounded read did not finish')
            answer = fixture(command)
            answer['root_id'] = req['payload']['root_id']
            answer['baseline']['finished_at'] = self.finished_at
            answer['largest_folders'][0]['node_id'] = 'nd_dir_1'
            if self.mode in ('none', 'failed') or req['payload']['root_id'] == 'rt_second':
                answer.update(outcome='no_baseline', baseline=None, new_files=None, new_dirs=None,
                              logical_bytes=None, allocated_bytes=None, allocation_unknown_files=None,
                              largest_files=[], largest_folders=[], caveats=[], partial=self.mode == 'failed',
                              partial_reasons=['baseline_not_completed'] if self.mode == 'failed' else [])
            elif self.mode == 'zero':
                answer.update(new_files=0, new_dirs=0, logical_bytes='0', allocated_bytes='0',
                              allocation_unknown_files=0, largest_files=[], largest_folders=[])
            elif self.mode == 'partial':
                answer.update(partial=True, partial_reasons=['coverage_partial', 'changes_since_scan'])
            return answer
        return super().result(req)


def run_new_since(browser):
    for zone, instant, expected_date in [
        ('America/Los_Angeles', '2026-10-23T00:30:00Z', '2026-10-22'),
        ('Pacific/Auckland', '2026-10-23T23:30:00Z', '2026-10-24'),
    ]:
        engine = ComparisonEngine()
        engine.finished_at = instant
        engine.start()
        context = browser.new_context(timezone_id=zone)
        page = context.new_page()
        case = 0
        try:
            def load(mode):
                nonlocal case
                case += 1
                engine.mode = mode
                # A new query forces a new document/session. Changing only the
                # token fragment can leave the previous Today response mounted.
                with page.expect_response(lambda response: response.request.method == 'POST'
                                          and response.url.endswith('/api/v3/call')
                                          and response.request.post_data_json['command'] == 'changes.new_since'):
                    page.goto(f'{engine.base}/?transport=http&comparison_case={case}#token={TOKEN}')
                expect(page.get_by_role('heading', name='Today', level=1)).to_be_visible()
                region = page.get_by_role('region', name='New since the last scan')
                expect(region).to_be_visible()
                return region

            region = load('known')
            expect(region).to_contain_text(f'the scan on {expected_date}:')
            region.get_by_role('link', name='Open the largest new folder in Explorer').click()
            expect(page.get_by_role('heading', name='Explorer', level=1)).to_be_visible()
            expect(page).to_have_url(re.compile(r'node=nd_dir_1'))
            wait_until(page, lambda: 'nd_dir_1' in engine.inspected)
            expect(load('zero')).to_contain_text('0 B in 0 files.')
            for mode in ('none', 'failed', 'missing'):
                region = load(mode)
                expect(region).to_contain_text('New since the last scan: not known.')
                expect(region.get_by_role('link')).to_have_count(0)
            region = load('partial')
            expect(region).to_contain_text('the folder is only partly scanned')
            expect(region).to_contain_text('the folder changed after its last scan')
            second = deepcopy(engine.roots[0])
            second['root_id'] = 'rt_second'
            engine.roots.append(second)
            region = load('known')
            expect(region).to_contain_text('At least ')
            expect(region).to_contain_text('not counted')
        finally:
            context.close()
            engine.stop()
    print('PASS Today comparisons: local dates, real zero, unknowns, partial watcher disclosure, lower bound and Explorer target')
