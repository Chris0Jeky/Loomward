"""Atlas view checks (lane L10). Run by scripts/test_app.py, which passes a context with the page.

Covers: load on the mock, P9 marks on a 2,500-node slice, hover and zoom frame pacing, keyboard drill
and back with an aria-live description, mouse drill and breadcrumb, the text list of regions,
hostile names as text, allocated-basis hatching, and screenshots at 1440 and 390 px.
"""
from __future__ import annotations

import json
import os

from playwright.sync_api import expect

HOSTILE = '<img src=x onerror=alert(1)>.png'

# Names with characters that change how text looks without showing (synth.ts corpus).
RLO_FILE = 'invoice\u202etxt.exe'
ZW_FILE = 'budget\u200b\u200dfinal.xlsx'
RLO_DIR = 'reports\u202etxt.exe'
ZW_DIR_FILE = 'scan\u200b\u200dresults.bin'
HIDDEN = ('\u202e', '\u200b', '\u200d')

# Records every string a canvas draws, so the test can read the "canvas label source".
FILLTEXT_RECORDER = """(() => {
  if (window.__fillTexts) return;
  const orig = CanvasRenderingContext2D.prototype.fillText;
  const seen = (window.__fillTexts = new Set());
  CanvasRenderingContext2D.prototype.fillText = function (t, ...rest) { if (seen.size < 50000) seen.add(String(t)); return orig.call(this, t, ...rest); };
})()"""


def drawn_texts(page) -> list[str]:
    return page.evaluate('[...(window.__fillTexts ?? [])]')


def reset_drawn(page) -> None:
    page.evaluate('window.__fillTexts && window.__fillTexts.clear()')


def drew_prefix_of(drawn: list[str], escaped: str) -> bool:
    # Canvas labels are fitted to their cell, so platform fonts change how much survives: accept any
    # drawn label that is a 3+ character prefix of the escaped form (the raw-character check is separate).
    return any(len(s) >= 3 and escaped.startswith(s) for s in (t.rstrip('…').rstrip('.') for t in drawn))


def badges(locator) -> list[str]:
    return locator.locator('.ctl').all_inner_texts()

FRAME_PACING = """async ({ kind, frames }) => {
  const c = document.querySelector('main canvas');
  const r = c.getBoundingClientRect();
  const gaps = []; let last = 0, i = 0;
  await new Promise((done) => {
    const tick = (t) => {
      if (last) gaps.push(t - last); last = t;
      if (kind === 'hover') {
        const x = r.left + ((i * 37) % 100) / 100 * r.width, y = r.top + ((i * 53) % 100) / 100 * r.height;
        c.dispatchEvent(new PointerEvent('pointermove', { clientX: x, clientY: y, bubbles: true }));
      }
      if (++i < frames) requestAnimationFrame(tick); else done();
    };
    requestAnimationFrame(tick);
  });
  gaps.sort((a, b) => a - b);
  const q = (p) => gaps[Math.min(gaps.length - 1, Math.floor(gaps.length * p))];
  return { frames: gaps.length, median_ms: q(0.5), p95_ms: q(0.95), fps_median: 1000 / q(0.5) };
}"""


def crumbs(page):
    return page.locator('main nav[aria-label="Location"] button')


def p9_marks(page, prefix: str) -> list[dict]:
    return page.evaluate(
        """(prefix) => ['layout', 'first-paint', 'slice-to-paint'].map((k) => {
             const e = performance.getEntriesByName(`${prefix}:${k}`);
             return e.map((m) => ({ name: k, ms: m.duration, nodes: m.detail?.nodes, cells: m.detail?.cells }));
           }).flat()""",
        prefix,
    )


def run(ctx) -> dict:
    page, base, check = ctx.page, ctx.base, ctx.check
    results: dict = {}

    def budget(ok: bool, label: str) -> None:
        # Frame-time budgets hold on the owner's machine; shared CI runners only report them.
        if os.environ.get('CI'):
            print(('PASS ' if ok else 'BUDGET (report only on CI) ') + label)
        else:
            check(ok, label)

    page.add_init_script(FILLTEXT_RECORDER)
    page.evaluate(FILLTEXT_RECORDER)  # a hash-only goto does not rerun init scripts
    page.goto(f'{base}/?transport=mock#/atlas')
    expect(page.get_by_role('heading', name='Atlas', level=1)).to_be_visible()
    canvas = page.locator('main canvas').first
    expect(canvas).to_be_visible()
    page.wait_for_function("performance.getEntriesByName('loomward:atlas:slice-to-paint').length > 0")
    expect(page.locator('main .status')).to_contain_text('2,500 nodes')
    expect(page.locator('main .status')).to_contain_text('synthetic')

    # --- P9: layout + first draw on the 2,500-node slice ----------------------------------------
    marks = p9_marks(page, 'loomward:atlas')
    first = {m['name']: m for m in marks if m['nodes'] == 2500}
    check({'layout', 'first-paint', 'slice-to-paint'} <= set(first), f'P9 marks recorded for the 2,500-node slice: {marks}')
    cpu = first['layout']['ms'] + first['first-paint']['ms']
    results['p9_first_slice'] = {'nodes': 2500, 'cells_drawn': first['layout']['cells'], 'layout_ms': round(first['layout']['ms'], 2),
                                 'first_paint_ms': round(first['first-paint']['ms'], 2), 'layout_plus_paint_ms': round(cpu, 2),
                                 'slice_to_paint_wall_ms': round(first['slice-to-paint']['ms'], 2)}
    budget(cpu <= 50, f'P9 layout + first draw of 2,500 nodes within 50 ms ({cpu:.1f} ms)')
    page.wait_for_timeout(1900)  # the loom reveal
    hover = page.evaluate(FRAME_PACING, {'kind': 'hover', 'frames': 90})
    results['p9_hover'] = hover
    budget(hover['fps_median'] >= 50, f"P9 hover at 50 fps or better (median {hover['fps_median']:.0f} fps)")

    # --- keyboard: cursor, aria-live, drill and back -------------------------------------------
    canvas.focus()
    page.keyboard.press('ArrowRight')
    expect(page.locator('#atlas-live')).not_to_have_text('')
    check(page.locator('main aside.inspector h2').count() == 1, 'the keyboard cursor fills the inspector')
    before = crumbs(page).count()
    page.keyboard.press('Home')
    zoom = page.evaluate("""async () => {
      const gaps = []; let last = 0, go = true;
      const tick = (t) => { if (last) gaps.push(t - last); last = t; if (go) requestAnimationFrame(tick); };
      requestAnimationFrame(tick);
      document.querySelector('main canvas').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
      await new Promise((r) => setTimeout(r, 900)); go = false;
      gaps.sort((a, b) => a - b);
      return { frames: gaps.length, median_ms: gaps[gaps.length >> 1], p95_ms: gaps[Math.floor(gaps.length * 0.95)], fps_median: 1000 / gaps[gaps.length >> 1] };
    }""")
    results['p9_zoom'] = zoom
    expect(crumbs(page)).to_have_count(before + 1)
    budget(zoom['fps_median'] >= 50, f"P9 zoom at 50 fps or better (median {zoom['fps_median']:.0f} fps)")
    canvas.focus()
    page.keyboard.press('Escape')
    expect(crumbs(page)).to_have_count(before)
    page.wait_for_timeout(900)  # let the zoom out settle: clicks during a zoom are ignored
    print('PASS atlas keyboard drill and back')

    # --- mouse drill and breadcrumb --------------------------------------------------------------
    box = canvas.bounding_box()
    page.mouse.click(box['x'] + box['width'] * 0.25, box['y'] + box['height'] * 0.5)
    expect(crumbs(page)).to_have_count(before + 1)
    page.wait_for_timeout(900)
    page.mouse.move(box['x'] + box['width'] * 0.5, box['y'] + box['height'] * 0.5)
    page.wait_for_timeout(200)
    check(page.locator('main .tip').count() == 1, 'hover shows a tooltip')
    ctx.view_shot('atlas-drilled-1440.png')
    crumbs(page).first.click()
    expect(crumbs(page)).to_have_count(before)
    print('PASS atlas mouse drill and breadcrumb back')

    # --- inspector path: every ancestor of a deep hover is listed (#141) ----------------------------
    box = canvas.bounding_box()
    trail = crumbs(page).all_inner_texts()
    deepest: list[str] = []
    for gy in range(1, 12):
        for gx in range(1, 16):
            page.mouse.move(box['x'] + box['width'] * gx / 16, box['y'] + box['height'] * gy / 12)
            parts = page.locator('main aside.inspector .path bdi').all_inner_texts()
            if len(parts) > len(deepest):
                deepest = parts
    check(len(deepest) >= len(trail) + 2, f'inspector path of a depth-2 hover lists the intermediate directory: {deepest}')
    check(deepest[:len(trail)] == trail, 'inspector path starts with the trail')
    page.mouse.move(0, 0)

    # --- text equivalent and hostile names -------------------------------------------------------
    page.locator('main details.as-text summary').click()
    page.locator('main details.as-text button', has_text='Synthetic Alpha').click()
    expect(crumbs(page).last).to_have_text('Synthetic Alpha')
    page.wait_for_timeout(700)
    lst = page.locator('main details.as-text')
    if not lst.evaluate('d => d.open'):
        page.locator('main details.as-text summary').click()
    check(lst.get_by_text(HOSTILE, exact=True).count() == 1, 'hostile file name listed as literal text')
    check(lst.get_by_text('<script>alert(document.domain)</script>.txt', exact=True).count() == 1, 'script-tag name listed as text')
    check(page.locator('main img, main script, main iframe, main svg').count() == 0, 'hostile names created no elements')
    check(not ctx.dialogs, 'no script from a file name ran')

    # --- hidden characters: badges in the DOM, escaped text on canvas and in aria-live -------------
    row = lst.locator('li', has_text='invoice')
    check('U+202E RLO' in badges(row), 'list: the right-to-left override shows as a U+202E badge')
    check('U+200B ZWSP' in badges(lst.locator('li', has_text='budget')), 'list: the zero-width space shows as a U+200B badge')
    row.get_by_role('button', name='Inspect').click()
    insp = page.locator('main aside.inspector h2')
    check('U+202E RLO' in badges(insp), 'inspector: the override shows as a badge in the name')
    lst.locator('li', has_text='budget').get_by_role('button', name='Inspect').click()
    check('U+200D ZWJ' in badges(insp), 'inspector: the zero-width joiner shows as a badge in the name')
    drawn = drawn_texts(page)
    check(drew_prefix_of(drawn, 'reportsU+202E RLOtxt.exe'), 'canvas label source: the hostile folder is drawn as its escaped form (fitted)')
    check(not any(h in t for t in drawn for h in HIDDEN), 'canvas never draws a raw hidden character')
    reset_drawn(page)
    lst.locator('li', has_text='reports').get_by_role('button').first.click()
    expect(crumbs(page)).to_have_count(3)
    check('U+202E RLO' in badges(crumbs(page).last), 'breadcrumb: the folder name shows its badge')
    page.wait_for_timeout(900)
    live_text = page.locator('#atlas-live').inner_text()
    check('U+202E' in live_text and not any(h in live_text for h in HIDDEN), 'aria-live speaks the escaped name')
    drawn = drawn_texts(page)
    check(drew_prefix_of(drawn, 'scanU+200B ZWSPU+200D ZWJresults.bin'), 'canvas label source: the zero-width file is drawn as its escaped form (fitted)')
    check(not any(h in t for t in drawn for h in HIDDEN), 'canvas never draws a raw hidden character (inside the folder)')
    box = canvas.bounding_box()
    page.mouse.move(box['x'] + box['width'] * 0.5, box['y'] + box['height'] * 0.5)
    expect(page.locator('main .tip .ctl').first).to_be_visible()
    check('U+200B ZWSP' in badges(page.locator('main .tip')), 'tooltip: the zero-width space shows as a badge')
    page.mouse.move(0, 0)
    crumbs(page).first.click()
    expect(crumbs(page)).to_have_count(1)
    page.wait_for_timeout(700)

    # --- allocated basis: unknown allocation is hatched and named, never zero ---------------------
    reset_drawn(page)
    page.get_by_role('radio', name='Allocated').click()
    expect(page.get_by_role('radio', name='Allocated')).to_have_attribute('aria-checked', 'true')
    page.wait_for_timeout(900)
    drawn = drawn_texts(page)
    check(any('alloc unknown' in t for t in drawn), 'allocated basis: label bands name unknown allocation on the cloth')
    if not lst.evaluate('d => d.open'):
        page.locator('main details.as-text summary').click()
    alpha = lst.locator('li', has_text='Synthetic Alpha')
    check('with unknown allocation' in alpha.inner_text() and 'at least' in alpha.inner_text(), 'allocated basis: the text list says "at least" and names the unknown files, never 0 B')
    alpha.get_by_role('button', name='Inspect').click()
    check('counted as 0 under this basis' in page.locator('main aside.inspector').inner_text(), 'allocated basis: the inspector explains the hatching')
    page.get_by_role('radio', name='Allocated').press('ArrowLeft')
    expect(page.get_by_role('radio', name='Logical size')).to_have_attribute('aria-checked', 'true')
    print('PASS atlas basis radiogroup with arrow keys')

    # --- provisional slice: a scanning root is woven loose and says so ---------------------------
    page.locator('main details.as-text button', has_text='Synthetic Beta').click()
    expect(page.locator('main .status')).to_contain_text('provisional')
    expect(page.locator('#atlas-live')).to_contain_text('provisional')  # the live text lands a beat after the slice (see Say)
    print('PASS aria-live says the slice is provisional')
    page.wait_for_timeout(800)
    ctx.view_shot('atlas-provisional-1440.png')
    crumbs(page).first.click()
    expect(crumbs(page)).to_have_count(1)

    # --- reduced motion: the view still works, with no reveal or zoom tween -----------------------
    page.emulate_media(reduced_motion='reduce')
    page.reload()
    expect(page.get_by_role('heading', name='Atlas', level=1)).to_be_visible()
    page.wait_for_function("performance.getEntriesByName('loomward:atlas:slice-to-paint').length > 0")
    canvas.focus()
    page.keyboard.press('Enter')
    expect(crumbs(page)).to_have_count(2)
    page.keyboard.press('Escape')
    expect(crumbs(page)).to_have_count(1)
    page.emulate_media(reduced_motion='no-preference')
    print('PASS atlas under reduced motion')

    # --- screenshots -------------------------------------------------------------------------------
    page.mouse.move(0, 0)
    page.wait_for_timeout(400)
    ctx.view_shot('atlas-1440.png')
    page.set_viewport_size({'width': 390, 'height': 844})
    page.reload()
    expect(page.get_by_role('heading', name='Atlas', level=1)).to_be_visible()
    page.wait_for_timeout(2300)
    check(page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'atlas: no horizontal scroll at 390 px')
    ctx.view_shot('atlas-390.png', full=False)
    page.set_viewport_size({'width': 1440, 'height': 900})
    print('P9 atlas', json.dumps(results))
    return results
