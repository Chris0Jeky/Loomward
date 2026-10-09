"""Observatory view checks (lane L10). Run by scripts/test_app.py, which passes a context with the page.

Covers: the sunburst drill by click and by keyboard, back through the centre, orbit by drag without
a focus change, synthetic telemetry labelled synthetic and moving, the memory read-out never zero
when unknown, hostile names as text, and screenshots at 1440 and 390 px.
"""
from __future__ import annotations

import math

from playwright.sync_api import expect

HOSTILE = '<img src=x onerror=alert(1)>.png'


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
    page.goto(f'{base}/?transport=mock#/observatory')
    expect(page.get_by_role('heading', name='Observatory', level=1)).to_be_visible()
    page.get_by_role('group', name='Theme').get_by_role('button', name='Observatory').click()  # its own identity
    canvas = page.locator('main .well canvas')
    expect(canvas).to_be_visible()
    expect(page.locator('main .res-note')).to_contain_text('Synthetic telemetry', timeout=5000)
    seq0 = page.locator('main .res-note').inner_text()
    page.wait_for_timeout(2200)
    check(page.locator('main .res-note').inner_text() != seq0, 'synthetic telemetry moves (new samples arrive)')
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
