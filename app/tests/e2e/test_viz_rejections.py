"""Rejected slices clear both real canvas renderers and use the app's load-error path."""
from pathlib import Path

from playwright.sync_api import expect


def run(ctx) -> dict:
    page, base, check = ctx.page, ctx.base, ctx.check
    viz = Path(__file__).resolve().parents[2] / 'viz'

    def module(route):
        route.fulfill(path=str(viz / route.request.url.rsplit('/', 1)[-1]), content_type='text/javascript')

    page.route('**/lens-viz/*.js', module)
    page.goto(f'{base}/?transport=mock#/atlas')
    try:
        results = page.evaluate("""async () => {
          const { createWovenTreemap } = await import('/lens-viz/woven-treemap.js');
          const { createSunburst } = await import('/lens-viz/sunburst.js');
          const palette = Object.fromEntries(['ink', 'frame', 'head', 'scrim', 'label', 'labelDim', 'fringe', 'loose',
            'cursor', 'hover', 'shuttle', 'track', 'scale', 'undyed', 'neutral', 'hollow', 'permission', 'unknown'].map(k => [k, '#123456']));
          Object.assign(palette, { dyes: ['#abcdef', '#fedcba'], metals: ['#abcdef', '#abcdef', '#abcdef'],
            fontDisplay: '16px sans-serif', fontHead: '12px sans-serif', fontLabel: '12px sans-serif', fontNum: '12px sans-serif', fontSmall: '12px sans-serif' });
          const node = (id, parent) => ({ node_id: id, parent, name: id, kind: 'dir', depth: 0, size_bytes: '10', logical_bytes: '10', allocated_bytes: '10',
            size_unknown_files: 0, files: 1, dirs: 0, child_count: 1, folded_count: null, coverage: 'complete', live: false,
            threads: { meaning: { state: 'none', label: null }, residency: { tier: null, volume_id: null, tier_basis: 'unknown' }, permission: { state: 'granted', reason: null } } });
          const good = { anchor_node_id: 'root', basis: 'logical', complete: true, truncated: false, nodes: [node('root', null), node('child', 0)] };
          const out = [];
          for (const make of [createWovenTreemap, createSunburst]) {
            const c = document.createElement('canvas'); c.style.cssText = 'width:400px;height:300px'; c.tabIndex = 0; document.body.append(c);
            const renderer = make(c, { palette, reducedMotion: () => true });
            let interactions = 0;
            for (const event of ['drill', 'select']) renderer.on(event, () => interactions++);
            try {
              for (const nodes of [[], [node('root', null), node('child', 5)]]) {
                renderer.setSlice(good); renderer.renderFrame();
                if (!renderer.info('child')) throw new Error('control slice was not adopted');
                let refused = false;
                try { renderer.setSlice({ ...good, nodes }); } catch { refused = true; }
                renderer.renderFrame();
                await new Promise(requestAnimationFrame);
                const before = interactions;
                c.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }));
                c.dispatchEvent(new MouseEvent('click', { clientX: c.getBoundingClientRect().left + 200, clientY: c.getBoundingClientRect().top + 150 }));
                const pixels = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
                let blank = true;
                for (let i = 4; i < pixels.length; i += 4) {
                  if (pixels[i] !== pixels[0] || pixels[i+1] !== pixels[1] || pixels[i+2] !== pixels[2] || pixels[i+3] !== pixels[3]) { blank = false; break; }
                }
                out.push({ refused, cleared: renderer.info('child') === null, blank, inert: before === interactions });
              }
            } finally { renderer.destroy(); c.remove(); }
          }
          return out;
        }""")
        check(all(all(result.values()) for result in results), f'both renderers refuse empty and malformed slices, clear paint, and become inert: {results}')
    finally:
        page.unroute('**/lens-viz/*.js', module)

    # A malformed successful transport response must reach LoadError, not an uncaught effect.
    bad = False

    def reply(route):
        req = route.request.post_data_json
        if req['command'] != 'tree.slice':
            route.continue_()
            return
        threads = {'meaning': {'state': 'none', 'label': None}, 'residency': {'tier': None, 'volume_id': None, 'tier_basis': 'unknown'}, 'permission': {'state': 'granted', 'reason': None}}
        node = {'node_id': 'atlas', 'parent': None, 'name': 'Synthetic anchor', 'kind': 'atlas', 'depth': 0, 'size_bytes': '10', 'logical_bytes': '10', 'allocated_bytes': '10',
                'size_unknown_files': 0, 'files': 1, 'dirs': 0, 'child_count': 1, 'folded_count': None, 'coverage': 'complete', 'live': False, 'threads': threads}
        result = {'anchor_node_id': 'atlas', 'basis': req['payload']['basis'], 'complete': True, 'truncated': False, 'aggregate_state': 'consistent',
                  'nodes': [] if bad else [node, {**node, 'node_id': 'child', 'parent': 0, 'name': 'Synthetic child'}]}
        route.fulfill(json={'protocol': 'loomward/3', 'request_id': req['request_id'], 'ok': True, 'result': result,
                            'meta': {'served_at': '2026-10-01T00:00:00Z', 'elapsed_ms': 0, 'dataset_class': 'synthetic', 'budget_hit': False, 'catalog_rev': None, 'state_rev': None}})

    page.route('**/api/v3/call', reply)
    try:
        for view in ['atlas', 'observatory']:
            bad = False
            page.goto(f'{base}/#token=a1b2c3d4e5f60718293a4b5c6d7e8f90')
            page.evaluate(f"location.hash = '#/{view}'")
            expect(page.locator('main .status' if view == 'atlas' else 'main .help')).to_contain_text('2 nodes')
            page.locator('main details.as-text summary').click()
            page.locator('main details.as-text').get_by_role('button', name='Inspect Synthetic child', exact=True).click()
            expect(page.locator('main aside.inspector h2')).to_have_text('Synthetic child')
            bad = True
            page.get_by_role('radio', name='Allocated').click()
            expect(page.locator('main [role="alert"]')).to_contain_text('empty or malformed slice')
            expect(page.locator('main aside.inspector h2')).to_have_count(0)
            expect(page.locator('main nav[aria-label="Location"] button')).to_have_count(1)
            check(True, f'{view}: rejected response surfaces LoadError and drops the selection')
    finally:
        page.unroute('**/api/v3/call', reply)
    return {'renderer_rejections': results}
