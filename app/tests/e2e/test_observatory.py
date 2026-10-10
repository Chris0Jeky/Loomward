"""Observatory view checks (lane L10). Run by scripts/test_app.py, which passes a context with the page.

Covers: the sunburst drill by click and by keyboard, back through the centre, orbit by drag without
a focus change, synthetic telemetry labelled synthetic and moving, the memory read-out never zero
when unknown, hostile names as text, and screenshots at 1440 and 390 px.
"""
from __future__ import annotations

import math

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


def crumbs(page):
    return page.locator('main nav[aria-label="Location"] button')


def ring_point(box: dict, ring: float, angle: float) -> tuple[float, float]:
    # mirrors viz/sunburst.js geometry: R = min(W,H)/2 - 34, hole r0 = max(34, 0.24 R), four rings
    w, h = box['width'], box['height']
    R = max(40, min(w, h) / 2 - 34)
    r0 = max(34, R * 0.24)
    T = (R - r0) / 4
    r = r0 + T * ring
    return box['x'] + w / 2 + r * math.cos(angle), box['y'] + h / 2 + r * math.sin(angle)


def run(ctx) -> dict:
    page, base, check = ctx.page, ctx.base, ctx.check
    page.add_init_script(FILLTEXT_RECORDER)
    page.evaluate(FILLTEXT_RECORDER)  # a hash-only goto does not rerun init scripts
    page.goto(f'{base}/?transport=mock#/observatory')
    expect(page.get_by_role('heading', name='Observatory', level=1)).to_be_visible()
    page.get_by_role('group', name='Theme').get_by_role('button', name='Observatory').click()  # its own identity
    canvas = page.locator('main .well canvas')
    expect(canvas).to_be_visible()
    expect(page.locator('main .res-note')).to_contain_text('Synthetic telemetry', timeout=5000)
    seq0 = page.locator('main .res-note').inner_text()
    page.wait_for_timeout(2200)
    check(page.locator('main .res-note').inner_text() != seq0, 'synthetic telemetry moves (new samples arrive)')
    status = page.locator('main .res-note [role="status"]').inner_text()
    check('sample' not in status, 'the live status announces state only, not every sample number')
    labels = page.locator('main .disks .dl').all_inner_texts()
    check(len(labels) >= 1 and all('Disk' in l for l in labels), f'disk rows are named by the sample itself: {labels}')
    mem = page.locator('main dl.mem').inner_text()
    check('unknown' not in mem.split('Commit')[0], 'memory read-out is filled from the sample')
    check('cleaner' in page.locator('main .resources').inner_text(), 'no cleaner theatre: the read-only note is present')

    # --- drill by click on the first ring, back through the centre --------------------------------
    box = canvas.bounding_box()
    page.wait_for_timeout(400)
    start = crumbs(page).count()
    x, y = ring_point(box, 0.5, -math.pi / 2 + 0.15)
    page.mouse.click(x, y)
    expect(crumbs(page)).to_have_count(start + 1)
    page.wait_for_timeout(900)
    # orbit: a drag rotates and must not change where we are
    page.mouse.move(box['x'] + box['width'] * 0.85, box['y'] + box['height'] / 2)
    page.mouse.down()
    page.mouse.move(box['x'] + box['width'] / 2, box['y'] + box['height'] * 0.85, steps=8)
    page.mouse.up()
    page.wait_for_timeout(600)
    expect(crumbs(page)).to_have_count(start + 1)
    x, y = ring_point(box, 0.5, 0.4)
    page.mouse.move(x, y)
    page.wait_for_timeout(250)
    ctx.view_shot('observatory-drilled-1440.png')
    cx, cy = box['x'] + box['width'] / 2, box['y'] + box['height'] / 2
    page.mouse.click(cx, cy)
    expect(crumbs(page)).to_have_count(start)
    print('PASS observatory click drill, orbit, centre back')

    # --- keyboard -----------------------------------------------------------------------------------
    page.wait_for_timeout(800)
    canvas.focus()
    page.keyboard.press('ArrowRight')
    expect(page.locator('#obs-live')).not_to_have_text('')
    page.keyboard.press('Home')
    page.keyboard.press('Enter')
    expect(crumbs(page)).to_have_count(start + 1)
    page.wait_for_timeout(900)
    canvas.focus()
    page.keyboard.press('Escape')
    expect(crumbs(page)).to_have_count(start)
    print('PASS observatory keyboard drill and back')

    # --- hostile names as text -------------------------------------------------------------------------
    page.locator('main details.as-text summary').click()
    page.locator('main details.as-text button', has_text='Synthetic Alpha').click()
    expect(crumbs(page).last).to_have_text('Synthetic Alpha')
    page.wait_for_timeout(900)
    lst = page.locator('main details.as-text')
    if not lst.evaluate('d => d.open'):
        page.locator('main details.as-text summary').click()
    check(lst.get_by_text(HOSTILE, exact=True).count() == 1, 'observatory lists the hostile name as literal text')
    check(page.locator('main img, main script, main iframe, main svg').count() == 0, 'observatory: hostile names created no elements')
    check('U+202E RLO' in badges(lst.locator('li', has_text='invoice')), 'observatory list: override badge')
    check('U+200B ZWSP' in badges(lst.locator('li', has_text='budget')), 'observatory list: zero-width badge')
    lst.locator('li', has_text='invoice').get_by_role('button', name='Inspect').click()
    check('U+202E RLO' in badges(page.locator('main aside.inspector h2')), 'observatory inspector: override badge')
    drawn = drawn_texts(page)
    check(not any(h in t for t in drawn for h in HIDDEN), 'observatory canvas never draws a raw hidden character')
    lst.locator('li', has_text='reports').get_by_role('button').first.click()
    expect(crumbs(page)).to_have_count(3)
    check('U+202E RLO' in badges(crumbs(page).last), 'observatory breadcrumb: override badge')
    page.wait_for_timeout(1000)
    drawn = drawn_texts(page)
    check(drew_prefix_of(drawn, 'reportsU+202E RLOtxt.exe'), 'observatory canvas label source: the folder is drawn as its escaped form (fitted)')
    check(not any(h in t for t in drawn for h in HIDDEN), 'observatory canvas never draws a raw hidden character (inside)')
    check('U+202E' in page.locator('#obs-live').inner_text(), 'observatory aria-live speaks the escaped name')
    crumbs(page).first.click()
    expect(crumbs(page)).to_have_count(1)

    # --- screenshots -----------------------------------------------------------------------------------
    page.mouse.move(0, 0)
    page.wait_for_timeout(900)
    ctx.view_shot('observatory-1440.png')
    page.set_viewport_size({'width': 390, 'height': 844})
    page.reload()
    expect(page.get_by_role('heading', name='Observatory', level=1)).to_be_visible()
    page.wait_for_timeout(1800)
    check(page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'observatory: no horizontal scroll at 390 px')
    ctx.view_shot('observatory-390.png', full=False)
    page.set_viewport_size({'width': 1440, 'height': 900})
    page.get_by_role('group', name='Theme').get_by_role('button', name='Woven atlas').click()
    return {}
