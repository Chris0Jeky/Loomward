"""Real synthetic NTFS metadata observation through the production HTTP/Svelte journey."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from playwright.sync_api import expect

ROOT = Path(__file__).resolve().parents[1]


def live_reclaim(browser, exe, dist, check, live_call):
    if sys.platform != 'win32':
        print('UNVERIFIED native reclaim browser leg: requires non-elevated Windows NTFS')
        return
    subprocess.run(['cargo', 'build', '-q', '-p', 'loomward-service', '--example', 'reclaim_selection_fixture'], cwd=ROOT, check=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target')
    fixture_exe = target / 'debug/examples/reclaim_selection_fixture.exe'
    roots = ROOT / 'target/test-roots'
    roots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='reclaim-selection-', dir=roots) as temporary:
        state, folder = Path(temporary) / 'state', Path(temporary) / 'selected-lab'
        subprocess.run([str(fixture_exe), str(state), str(folder)], cwd=ROOT, check=True)
        names = ['Cargo.toml', 'target/shared.bin', 'target/own.bin', '.cargo/registry/cache/shared.bin']
        def snapshot():
            return {name: ((folder / name).read_bytes(), (folder / name).stat().st_mtime_ns, (folder / name).stat().st_nlink) for name in names}
        before = snapshot()
        proc = subprocess.Popen([str(exe), '--state-dir', str(state), '--static', str(dist), '--grant-root', str(folder)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        context = None
        try:
            url = proc.stdout.readline().strip()
            check('/#token=' in url, 'native selection: registered synthetic service started')
            token = url.split('/#token=')[1]
            context = browser.new_context(viewport={'width': 1440, 'height': 900})
            page = context.new_page()
            page.set_default_timeout(15000)
            page.goto(url)
            expect(page.get_by_role('status', name='Session status')).to_contain_text('Dataset synthetic')
            page.evaluate("location.hash = '#/health'")
            page.locator('ul.scans li').get_by_role('button', name='Scan selected-lab', exact=True).click()
            expect(page.locator('ul.scans li .job')).to_contain_text('Finished, complete coverage', timeout=30000)
            page.evaluate("location.hash = '#/reclaim'")
            expect(page.get_by_role('checkbox')).to_have_count(2)
            page.get_by_role('button', name='Observe hard links', exact=True).click()
            expect(page.get_by_label('Observation status')).to_contain_text('Observation finished')
            volume = page.get_by_label('Volume to review').input_value()
            plan = live_call(page, token, 'reclaim.plan', {'volume_id': volume, 'limit': 50})['result']
            candidates = [c for group in plan['groups'] for c in group['candidates']]
            check(len(candidates) == 2 and all(c['reclaimable_bytes'] is None for c in candidates), 'native selection: each candidate individually has an external link')
            for checkbox in page.get_by_role('checkbox').all():
                checkbox.focus()
                page.keyboard.press('Space')
            page.get_by_label('Space target (bytes)', exact=True).fill('20480')
            page.get_by_role('button', name='Observe selection', exact=True).click()
            result = page.get_by_label('Selection estimate', exact=True)
            expect(result).to_contain_text('20480 bytes')
            expect(result).to_contain_text('covers the target, with 0 bytes above')
            selection = [c['node_id'] for c in candidates]
            recorded = live_call(page, token, 'reclaim.plan', {'volume_id': volume, 'limit': 50, 'selection': selection})['result']['selection']
            check(recorded['status'] == 'recorded' and recorded['reclaimable_bytes'] == '20480', 'native selection: joint set counts the shared NTFS object once')
            # Independent synthetic oracle, only in the test process; the app never starts Python.
            sys.path.insert(0, str(ROOT))
            from experiments.reclaim_union import Candidate, FileFact, Scenario, relief, validate
            scenario = Scenario((FileFact('v', 'shared', 8192, 8192, True, 2, ('target_shared', 'cache_shared')),
                                 FileFact('v', 'own', 12288, 12288, True, 1, ('target_own',))),
                                (Candidate('target', 'v', ('target_shared', 'target_own')), Candidate('cache', 'v', ('cache_shared',))), {'v': 20480})
            validate(scenario)
            oracle = relief(scenario, ('target', 'cache'))
            check(int(recorded['reclaimable_bytes']) == oracle['guaranteed_bytes']['v'] == oracle['possible_bytes']['v'], 'native selection: complete case agrees with independent PR1001 oracle')
            evidence = ROOT / 'target/reclaim-ui-evidence'
            evidence.mkdir(parents=True, exist_ok=True)
            for width in [1440, 390]:
                page.set_viewport_size({'width': width, 'height': 900})
                check(page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'native selection: fits {width}px')
                page.screenshot(path=str(evidence / f'selection-{width}.png'), full_page=True)
            check(snapshot() == before, 'native selection: contents, modification times and hard links unchanged')
            (evidence / 'native-selection.json').write_text(json.dumps({'case': 'reclaim-selection-1', 'dataset': 'synthetic', 'native': True,
                'individual_bytes': [None, None], 'selected_bytes': recorded['reclaimable_bytes'], 'oracle_bytes': oracle['guaranteed_bytes']['v'],
                'observed_at': recorded['observed_at'], 'source_files_unchanged': True}, indent=2) + '\n', encoding='utf-8')
            print('PASS native Reclaim selection: two individually unknown candidates, closed joint links, exact target, independent oracle, no source effect')
        finally:
            if context:
                context.close()
            if proc.poll() is None:
                proc.terminate()
            proc.wait(timeout=10)
