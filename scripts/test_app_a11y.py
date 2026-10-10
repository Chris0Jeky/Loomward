"""WCAG 2.2 AA checks for the Svelte app in app/ (lane L18, issue #157, follow-ups of #156).

Run with `py -3 scripts/test_app.py --a11y` after `npm.cmd --prefix app run build`. Everything runs on the
mock transport against the built app/dist; each check names the success criterion it pins. It measures what
a script can measure (names, focus, geometry, contrast, forced-colours styles). What a screen reader
actually says is NOT proven here: that needs NVDA or Narrator (see the list in the issue).
"""
from __future__ import annotations

import re
from pathlib import Path

from playwright.sync_api import Browser, Page, expect

ROOT = Path(__file__).resolve().parents[1]
TOKENS = ROOT / 'app' / 'src' / 'styles' / 'tokens.css'
INTERACTIVE = 'button|link|checkbox|radio|combobox|textbox|searchbox|spinbutton|tab|switch|slider|menuitem'


# --- contrast ---------------------------------------------------------------------------------------------

def _lin(c: float) -> float:
    c /= 255
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4


def _lum(rgb: tuple[int, int, int]) -> float:
    r, g, b = rgb
    return 0.2126 * _lin(r) + 0.7152 * _lin(g) + 0.0722 * _lin(b)


def ratio(a: tuple[int, int, int], b: tuple[int, int, int]) -> float:
    la, lb = sorted((_lum(a), _lum(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def hex_rgb(h: str) -> tuple[int, int, int]:
    h = h.lstrip('#')
    return int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)


def css_rgb(s: str) -> tuple[int, int, int]:
    m = re.match(r'rgba?\((\d+),\s*(\d+),\s*(\d+)', s)
    assert m, f'not a colour: {s}'
    return int(m[1]), int(m[2]), int(m[3])


def tokens() -> dict[str, dict[str, str]]:
    src = TOKENS.read_text(encoding='utf-8')
    blocks = {
        'woven-atlas': re.search(r":root,\s*:root\[data-theme='woven-atlas'\] \{(.*?)\n\}", src, re.S),
        'observatory': re.search(r":root\[data-theme='observatory'\] \{(.*?)\n\}", src, re.S),
    }
    return {t: dict(re.findall(r'--([\w-]+):\s*(#[0-9a-fA-F]{6})', m[1])) for t, m in blocks.items() if m}


# --- helpers ------------------------------------------------------------------------------------------------

def goto(page: Page, base: str, view: str, heading: str) -> None:
    page.goto(f'{base}/?transport=mock#/{view}')
    expect(page.get_by_role('heading', name=heading, level=1)).to_be_visible()


def active(page: Page) -> str:
    return page.evaluate("(() => { const a = document.activeElement; return a ? `${a.tagName}|${(a.textContent || '').trim().slice(0, 60)}` : ''; })()")


CONTEXTS = {'navigation', 'group', 'radiogroup', 'region', 'complementary', 'form', 'dialog', 'banner', 'contentinfo'}


def duplicate_names(page: Page) -> list[str]:
    """Interactive controls that share a role and an accessible name inside the same landmark or group (2.4.6, 4.1.2).

    Rows, list items and table cells do not count as context: the Inspect button of every row has to be told apart
    by its own name. Built from the accessibility tree (aria_snapshot), the nesting from its indentation."""
    seen: dict[tuple, int] = {}
    stack: list[tuple[int, str, str]] = []
    for line in page.locator('body').aria_snapshot().splitlines():
        m = re.match(r'^(\s*)- (\w+)(?:\s+"((?:[^"\\]|\\.)*)")?', line)
        if not m:
            continue
        indent, role, name = len(m[1]), m[2], m[3] or ''
        while stack and stack[-1][0] >= indent:
            stack.pop()
        if re.fullmatch(INTERACTIVE, role) and name:
            ctx = tuple((r, n) for _, r, n in stack if r in CONTEXTS)
            seen[(ctx, role, name)] = seen.get((ctx, role, name), 0) + 1
        stack.append((indent, role, name))
    return [f'{r} "{n}" x{c} in {[x[1] or x[0] for x in ctx]}' for (ctx, r, n), c in seen.items() if c > 1]


def dangling_refs(page: Page) -> list[str]:
    return page.evaluate("""() => [...document.querySelectorAll('[aria-labelledby],[aria-describedby],[aria-controls]')].flatMap((el) =>
      ['aria-labelledby', 'aria-describedby', 'aria-controls'].flatMap((a) => (el.getAttribute(a) || '').split(/\\s+/).filter(Boolean)
        .filter((id) => !document.getElementById(id)).map((id) => `${el.tagName}[${a}=${id}]`)))""")


def tab_to(page: Page, selector: str, limit: int = 60) -> bool:
    for _ in range(limit):
        page.keyboard.press('Tab')
        if page.evaluate('(s) => document.activeElement && document.activeElement.matches(s)', selector):
            return True
    return False


def cursor_roundtrip(page: Page, live, crumbs, check) -> None:
    """Open a region that is not the first with the keyboard, go back, and the cursor is still on it (announced on refocus)."""
    drilled = ''
    for k in range(1, 6):
        page.keyboard.press('Home')
        for _ in range(k):
            page.keyboard.press('ArrowRight')
        page.wait_for_timeout(200)
        page.keyboard.press('Enter')
        page.wait_for_timeout(300)
        if crumbs.count() == 2:
            drilled = crumbs.last.inner_text()
            break
    check(bool(drilled), f'a non-first region could be opened with the keyboard ({drilled})')
    page.wait_for_timeout(1000)
    page.keyboard.press('Escape')
    expect(crumbs).to_have_count(1)
    page.wait_for_timeout(1000)
    page.keyboard.press('Shift+Tab')
    page.keyboard.press('Tab')
    expect(live).to_contain_text(drilled)
    check(True, f'#157 back lands the keyboard cursor on the region just left ("{drilled}")')


def run_a11y(browser: Browser, base: str, check) -> None:
    errors: list[str] = []

    def new_page(width: int = 1280, height: int = 800, **kw) -> Page:
        ctx = browser.new_context(viewport={'width': width, 'height': height}, **kw)
        p = ctx.new_page()
        p.set_default_timeout(8000)
        p.on('pageerror', lambda e: errors.append(f'pageerror: {e}'))
        p.on('console', lambda m: m.type == 'error' and errors.append(f'console: {m.text}'))
        return p

    # --- 1.4.10 / 1.4.4 / 2.4.11: the chrome does not eat a zoomed viewport ------------------------------------
    for w, h, label in ((320, 256, '400% zoom'), (640, 360, '200% zoom'), (1280, 400, 'a short window')):
        page = new_page(w, h)
        goto(page, base, 'tiers', 'Tiers')
        m = page.evaluate("""() => { const el = document.querySelector('.masthead'), r = el.getBoundingClientRect(), p = getComputedStyle(el).position;
            const rail = getComputedStyle(document.querySelector('.rail')).position; return { h: r.height, vh: innerHeight, p, rail }; }""")
        pinned = m['h'] / m['vh'] if m['p'] in ('sticky', 'fixed') else 0.0
        check(pinned < 0.30, f"1.4.10 masthead pins {pinned:.0%} of the viewport at {w}x{h} ({label}); position {m['p']}, height {m['h']:.0f}px")
        check(m['rail'] not in ('sticky', 'fixed'), f"1.4.10 the rail does not pin at {w}x{h}")
        page.context.close()

    page = new_page(1280, 800)
    goto(page, base, 'tiers', 'Tiers')
    m = page.evaluate("() => ({ p: getComputedStyle(document.querySelector('.masthead')).position, h: document.querySelector('.masthead').getBoundingClientRect().height })")
    check(m['p'] == 'sticky' and m['h'] / 800 < 0.30, f"the masthead stays sticky on a roomy window and takes {m['h'] / 800:.0%} of it")
    # 2.4.11: a focused control is never under the sticky masthead (scroll-padding on the root)
    hidden_under = page.evaluate("""() => {
      const mh = document.querySelector('.masthead').getBoundingClientRect().bottom, out = [];
      for (const el of document.querySelectorAll('main button:not(:disabled), main input, main select, main a[href], .rail a')) {
        const r0 = el.getBoundingClientRect(); if (!r0.width || !r0.height) continue;
        window.scrollTo(0, window.scrollY + r0.top - 20); // the control sits just below the top edge, under a sticky masthead
        el.focus(); const r = el.getBoundingClientRect();
        if (r.top < mh - 0.5 && r.bottom > 0) out.push(`${el.tagName} ${(el.textContent || el.getAttribute('aria-label') || '').trim().slice(0, 30)} top ${r.top.toFixed(0)} < ${mh.toFixed(0)}`);
      }
      return out;
    }""")
    check(not hidden_under, f'2.4.11 no focused control sits under the sticky masthead: {hidden_under[:3]}')
    page.context.close()

    # --- unique names, row headers, dangling references (2.4.6, 4.1.2, 1.3.1) ---------------------------------------
    page = new_page()
    for view, heading in (('explorer', 'Explorer'), ('atlas', 'Atlas'), ('observatory', 'Observatory'), ('tiers', 'Tiers'), ('companion', 'Companion'), ('health', 'Grants & health')):
        goto(page, base, view, heading)
        expect(page.locator('main canvas, main table, main .vols').first).to_be_visible()
        if view in ('atlas', 'observatory'):
            expect(page.locator('main details.as-text li').first).to_be_attached()
            page.locator('main details.as-text summary').click()
            n = page.locator('main details.as-text button.inspect').count()
            check(n >= 2, f'{view}: the region list has {n} Inspect buttons to tell apart')
        if view == 'tiers':
            expect(page.get_by_role('button', name='Simulate', exact=True)).to_be_enabled()
            page.get_by_role('button', name='Simulate', exact=True).click()
            expect(page.get_by_role('heading', name='Alternatives')).to_be_visible()
        if view == 'health':
            expect(page.get_by_role('button', name=re.compile('^Revoke…')).first).to_be_visible()
            n = page.get_by_role('button', name=re.compile('^Revoke…')).count()
            check(n >= 2, f'health: {n} Revoke… triggers to tell apart')
        if view == 'companion':
            expect(page.locator('main table.procs tbody tr').first).to_be_visible()
        dups = duplicate_names(page)
        check(not dups, f'{view}: no two controls share a role and an accessible name {dups[:4]}')
        check(not dangling_refs(page), f'{view}: no aria-labelledby, describedby or controls points at a missing id: {dangling_refs(page)}')
        if view in ('explorer', 'tiers', 'companion', 'health'):
            heads = page.evaluate("[...document.querySelectorAll('main table tbody tr')].filter((tr) => tr.children.length > 2 && tr.firstElementChild.tagName === 'TD').length")
            check(heads == 0, f'{view}: the first cell of every data row is a row header (th scope=row); {heads} rows still start with td')
    # role=status must not replace the list semantics of the session chips
    check(page.evaluate("document.querySelector('ul.status').getAttribute('role')") is None, '1.3.1 the session chip list keeps its list role')
    check(page.get_by_role('status', name='Session status').locator('ul.status li').count() == 4, '4.1.3 the session status is a status region around the list')
    page.context.close()

    # --- 1.4.11: control edges ---------------------------------------------------------------------------------------
    toks = tokens()
    for theme, v in toks.items():
        edge = v.get('control-line')
        check(edge is not None, f'{theme}: tokens.css defines --control-line')
        if edge is None:
            continue
        worst = min(ratio(hex_rgb(edge), hex_rgb(v[bg])) for bg in ('bg', 'surface', 'raised'))
        check(worst >= 3.0, f'1.4.11 {theme}: --control-line is {worst:.2f}:1 or more on bg, surface and raised (needs 3:1)')
    page = new_page()
    for theme, label in (('woven-atlas', 'Woven atlas'), ('observatory', 'Observatory')):
        goto(page, base, 'explorer', 'Explorer')
        page.get_by_role('button', name=label, exact=True).click()
        panel_bg = css_rgb(page.evaluate("getComputedStyle(document.querySelector('main .panel')).backgroundColor"))
        page_bg = hex_rgb(toks[theme]['bg'])
        low = []
        for sel in ('main button.btn', 'main input[type=search]', 'main select', '.masthead .themes button'):
            col = css_rgb(page.evaluate("(s) => getComputedStyle(document.querySelector(s)).borderTopColor", sel))
            r = min(ratio(col, panel_bg), ratio(col, page_bg))
            if r < 3.0:
                low.append(f'{sel} {r:.2f}')
        check(not low, f'1.4.11 {theme}: button, input, select and theme-toggle edges are 3:1 against their surroundings {low}')
        goto(page, base, 'atlas', 'Atlas')
        col = css_rgb(page.evaluate("getComputedStyle(document.querySelector('.seg')).borderTopColor"))
        check(ratio(col, page_bg) >= 3.0, f'1.4.11 {theme}: the basis switch edge is {ratio(col, page_bg):.2f}:1')
    page.context.close()

    # --- 2.4.3 / 4.1.3: focus and announcements ------------------------------------------------------------------------
    page = new_page()
    goto(page, base, 'tiers', 'Tiers')
    sim = page.get_by_role('button', name='Simulate', exact=True)
    expect(sim).to_be_enabled()
    sim.click()
    expect(page.get_by_role('heading', name='Alternatives')).to_be_visible()
    expect(page.get_by_role('status').filter(has_text='Simulation finished')).to_be_attached()
    check(active(page).startswith('BUTTON|Simulate'), f'2.4.3 focus stays on Simulate after the result arrives ({active(page)})')
    page.get_by_role('button', name=re.compile('^Show')).nth(2).click()
    expect(page.get_by_role('status').filter(has_text='proposed moves')).to_be_attached()
    check(active(page).startswith('BUTTON|Show'), f'2.4.3 focus stays on the Show button that was pressed ({active(page)})')

    goto(page, base, 'health', 'Grants & health')
    trigger = page.get_by_role('button', name=re.compile('^Revoke…')).first
    trigger.click()
    group = page.get_by_role('group', name=re.compile('Revoke root access'))
    expect(group).to_be_focused()
    group.get_by_role('button', name='Keep it').click()
    check(active(page).startswith('BUTTON|Revoke'), f'2.4.3 Keep it returns focus to the Revoke… button ({active(page)})')
    page.get_by_role('button', name=re.compile('^Revoke…')).first.click()
    page.get_by_role('group', name=re.compile('Revoke root access')).get_by_role('button', name='Revoke', exact=True).click()
    expect(page.get_by_role('status').filter(has_text='Revoked')).to_be_visible()
    expect(page.locator('p.ok.live')).to_be_focused()
    check(not active(page).startswith('BODY'), f'2.4.3 focus is not dropped to the body after Revoke ({active(page)})')

    # the result text is read from a focused status: a hidden character in a root path is spelled out there too
    goto(page, base, 'health', 'Grants & health')
    row = page.locator('main section[aria-labelledby="h-roots"] tbody tr', has_text='reports')
    row.get_by_role('button', name=re.compile('^Revoke…')).click()
    page.get_by_role('group', name=re.compile('Revoke root access')).get_by_role('button', name='Revoke', exact=True).click()
    expect(page.locator('p.ok.live')).to_contain_text('Revoked')
    said = page.locator('p.ok.live').inner_text()
    check('U+202E' in said and '‮' not in said, f'4.1.3 invariant: the Revoke result spells the hidden character out, not raw: "{said[:60]}"')

    goto(page, base, 'companion', 'Companion')
    expect(page.locator('main table.procs tbody tr').first).to_be_visible()
    page.get_by_role('button', name=re.compile('^Explain')).first.click()
    expect(page.get_by_role('region', name='Explanation')).to_be_visible()
    check(active(page).startswith('H3|About'), f'4.1.3 Explain moves focus to the explanation heading ({active(page)})')

    goto(page, base, 'explorer', 'Explorer')
    expect(page.locator('main table tbody tr').first).to_be_visible()
    page.get_by_label('Search names').fill('script')
    page.get_by_role('button', name='Search', exact=True).click()
    expect(page.get_by_role('button', name='Clear search')).to_be_visible()
    page.get_by_role('button', name='Clear search').click()
    expect(page.get_by_label('Search names')).to_be_focused()
    check(True, '2.4.3 Clear search hands focus to the search box')

    # a route change lands on the new view's heading (nothing else announces it)
    page.get_by_role('navigation', name='Views').get_by_role('link', name='Atlas').click()
    expect(page.get_by_role('heading', name='Atlas', level=1)).to_be_focused()
    check(True, '2.4.3 a route change moves focus to the new heading')
    page.context.close()

    # --- 3.3.2: visible labels; 1.4.3: placeholders ----------------------------------------------------------------------
    page = new_page()
    goto(page, base, 'explorer', 'Explorer')
    labels = page.evaluate("""() => [...document.querySelectorAll('main form input, main form select')].map((el) => {
        const lab = el.closest('label'), t = lab ? [...lab.querySelectorAll('span')].find((s) => !s.classList.contains('sr-only')) : null;
        const r = t ? t.getBoundingClientRect() : null; return { ok: !!r && r.width > 0 && r.height > 0, name: el.getAttribute('placeholder') || el.tagName }; })""")
    check(all(x['ok'] for x in labels), f'3.3.2 every Explorer filter has a visible label {labels}')
    ph = css_rgb(page.evaluate("getComputedStyle(document.querySelector('input[type=search]'), '::placeholder').color"))
    bg = css_rgb(page.evaluate("getComputedStyle(document.querySelector('input[type=search]')).backgroundColor"))
    check(ratio(ph, bg) >= 4.5, f'1.4.3 the placeholder reads at {ratio(ph, bg):.2f}:1')

    # a hidden-character badge is introduced to assistive technology (CSS generated content is read, and stays out of innerText)
    goto(page, base, 'explorer', 'Explorer')
    expect(page.locator('main .ctl').first).to_be_attached()
    before = page.evaluate("getComputedStyle(document.querySelector('main .ctl'), '::before').content")
    check('hidden character' in before, f'badge: assistive technology hears "hidden character" before the code ({before})')
    check('hidden character' not in page.locator('main .ctl').first.inner_text(), 'badge: the visible text is still only the code')

    # --- 2.5.8: target size ---------------------------------------------------------------------------------------------
    for view, heading in (('explorer', 'Explorer'), ('atlas', 'Atlas'), ('tiers', 'Tiers'), ('companion', 'Companion'), ('health', 'Grants & health')):
        goto(page, base, view, heading)
        page.wait_for_timeout(300)
        small = page.evaluate("""() => [...document.querySelectorAll('button, a[href], select, input')].filter((el) => !el.closest('.skip') && !el.classList.contains('skip')).flatMap((el) => {
            const box = el.type === 'checkbox' || el.type === 'radio' ? (el.closest('label') || el) : el;
            const r = box.getBoundingClientRect(); if (!r.width || !r.height || r.x < 0) return [];
            return r.width < 24 - 0.5 || r.height < 24 - 0.5 ? [`${el.tagName} "${(el.textContent || el.getAttribute('aria-label') || '').trim().slice(0, 24)}" ${r.width.toFixed(0)}x${r.height.toFixed(0)}`] : [];
        })""")
        check(not small, f'2.5.8 {view}: every target is at least 24x24 px {small[:3]}')
    page.context.close()

    # --- Atlas: canvas keyboard, click-then-drill path (#156) ----------------------------------------------------------------
    page = new_page()
    goto(page, base, 'atlas', 'Atlas')
    expect(page.locator('main .status')).to_contain_text('nodes')
    page.wait_for_timeout(2200)  # the loom reveal
    crumbs = page.locator('main nav[aria-label="Location"] button')
    live = page.locator('#atlas-live')

    # click then drill through the text list: select X, then open X. The old inspector path repeated the last part.
    page.locator('main details.as-text summary').click()
    row = page.locator('main details.as-text li', has=page.locator('button.open')).first
    name = row.locator('button.open').inner_text().strip()
    row.locator('button.inspect').click()
    expect(page.locator('main aside.inspector h2')).to_have_text(name)
    row.locator('button.open').click()
    expect(crumbs).to_have_count(2)
    expect(page.locator('#threads-h')).to_be_visible()
    page.wait_for_timeout(300)
    parts = page.locator('main aside.inspector .path bdi').all_inner_texts()
    trail = crumbs.all_inner_texts()
    check(parts == trail, f'#156 click then drill: the inspector path is the trail, the last part is not repeated: {parts} vs {trail}')

    # a pointer click on a region selects it and opens it in one gesture: no part of the path repeats
    crumbs.first.click()
    expect(crumbs).to_have_count(1)
    page.wait_for_timeout(1000)
    box = page.locator('main canvas').first.bounding_box()
    page.mouse.click(box['x'] + box['width'] * 0.2, box['y'] + box['height'] * 0.4)
    expect(crumbs).to_have_count(2)
    page.mouse.move(0, 0)
    page.wait_for_timeout(1000)
    parts = page.locator('main aside.inspector .path bdi').all_inner_texts()
    check(all(a != b for a, b in zip(parts, parts[1:])), f'#156 a click that opens a region does not repeat a path part: {parts}')
    crumbs.first.click()
    expect(crumbs).to_have_count(1)
    page.wait_for_timeout(1000)

    # keyboard focus announces the region under the cursor (1.3.1 / 4.1.3 for a canvas)
    check(tab_to(page, 'main canvas'), 'the cloth is a Tab stop')
    expect(live).not_to_have_text('')
    expect(live).to_contain_text('Enter opens it')
    first = live.inner_text().split(', ')[0]
    check(bool(first), f'4.1.3 focusing the cloth announces the active region: "{live.inner_text()[:70]}"')
    # edge feedback: from the first region, one direction has no neighbour
    page.keyboard.press('Home')
    page.keyboard.press('ArrowLeft')
    page.keyboard.press('ArrowUp')
    expect(live).to_contain_text('Nothing further')
    check(True, '4.1.3 an arrow key with no region that way says so')
    # at the top level Escape has nowhere to go and says so
    page.keyboard.press('Escape')
    expect(live).to_contain_text('Already at the top')
    check(True, '4.1.3 Escape at the top level says so')

    cursor_roundtrip(page, live, crumbs, check)

    # 1.4.13: the hover tooltip can be dismissed with Escape
    box = page.locator('main canvas').first.bounding_box()
    page.mouse.move(box['x'] + box['width'] * 0.3, box['y'] + box['height'] * 0.4)
    page.wait_for_timeout(300)
    check(page.locator('main .tip').count() == 1, 'hovering a region shows the tooltip')
    page.keyboard.press('Escape')
    check(page.locator('main .tip').count() == 0, '1.4.13 Escape dismisses the tooltip')
    page.context.close()

    # --- Observatory: announcements, pause, gauge text -----------------------------------------------------------------------------
    page = new_page()
    goto(page, base, 'observatory', 'Observatory')
    expect(page.locator('main .orbit .help')).to_contain_text('nodes')
    page.wait_for_timeout(800)
    obs_crumbs = page.locator('main nav[aria-label="Location"] button')
    check(tab_to(page, 'main canvas'), 'the sunburst is a Tab stop')
    expect(page.locator('#obs-live')).to_contain_text('Enter opens it')
    cursor_roundtrip(page, page.locator('#obs-live'), obs_crumbs, check)
    page.locator('main details.as-text summary').click()
    row = page.locator('main details.as-text li', has=page.locator('button.inspect')).first
    row.locator('button.inspect').click()
    shown = page.locator('main aside.inspector h2').inner_text()
    expect(page.locator('#obs-live')).to_contain_text(shown.split(' ')[0])
    check(True, '4.1.3 Inspect in the Observatory speaks the region')
    gauges = page.evaluate("[...document.querySelectorAll('.gauges canvas')].map((c) => [c.getAttribute('role'), c.getAttribute('aria-label')])")
    check(len(gauges) == 4 and all(r == 'img' and l for r, l in gauges), f'1.1.1 the four gauges are images with a text alternative {gauges}')
    vram = [l for r, l in gauges if l.startswith('GPU memory')][0]
    check(' 0 B' not in vram and ('unknown' in vram or re.search(r'of \d', vram) is not None), f'invariant 4: the VRAM text alternative never says "0 B": "{vram}"')
    check(page.locator('main button', has_text='Rotate left').count() == 1 and page.locator('main button', has_text='Rotate right').count() == 1, '2.5.7 orbit has a single-pointer alternative to the drag')
    seq = lambda: page.locator('.res-note .seq').inner_text()
    expect(page.locator('.res-note .seq')).to_be_attached()
    page.get_by_role('button', name='Pause updates').click()
    expect(page.get_by_role('status').filter(has_text='Paused')).to_be_visible()
    before = seq()
    page.wait_for_timeout(2600)
    check(seq() == before, f'2.2.2 paused: the sample number holds ({before})')
    page.get_by_role('button', name='Resume updates').click()
    page.wait_for_timeout(2600)
    check(seq() != before, f'2.2.2 resumed: samples arrive again ({before} -> {seq()})')
    page.context.close()

    # --- Companion: no re-sort under focus, pause ------------------------------------------------------------------------------------
    page = new_page()
    goto(page, base, 'companion', 'Companion')
    expect(page.locator('main table.procs tbody tr').first).to_be_visible()
    page.get_by_label('Largest by').select_option('cpu_desc')
    page.wait_for_timeout(500)
    order = lambda: page.locator('main table.procs tbody th button').all_inner_texts()
    page.get_by_role('button', name=re.compile('^Explain')).nth(2).focus()
    held = order()
    page.wait_for_timeout(3600)
    check(order() == held, '2.2.2 rows do not re-sort while a control inside the table has focus')
    expect(page.get_by_text('A newer sample is waiting')).to_be_visible()
    page.get_by_role('heading', name='Companion', level=1).evaluate('el => { el.tabIndex = -1; el.focus(); }')
    expect(page.get_by_text('A newer sample is waiting')).to_have_count(0)
    page.get_by_role('button', name='Pause updates').click()
    expect(page.get_by_text('Paused: the table shows the last sample')).to_be_visible()
    held = order()
    page.wait_for_timeout(3600)
    check(order() == held, '2.2.2 paused: the table holds')
    page.context.close()

    # --- forced colours: current and checked states carry more than colour ---------------------------------------------------------------
    page = new_page(1280, 900, forced_colors='active')
    goto(page, base, 'atlas', 'Atlas')
    css = lambda sel, prop: page.evaluate("([s, p]) => getComputedStyle(document.querySelector(s))[p]", [sel, prop])
    check(css(".rail a[aria-current='page']", 'backgroundColor') != css('.rail a:not([aria-current])', 'backgroundColor'), '1.4.1 forced colours: the current rail link has its own background')
    check(css(".seg button[aria-checked='true']", 'backgroundColor') != css(".seg button[aria-checked='false']", 'backgroundColor'), '1.4.1 forced colours: the checked basis has its own background')
    check('underline' in css(".crumb[aria-current='location']", 'textDecorationLine'), '1.4.1 forced colours: the current crumb is underlined')
    goto(page, base, 'explorer', 'Explorer')
    check('underline' in css('th.basis', 'textDecorationLine') and 'underline' not in css('thead th:first-child', 'textDecorationLine'), '1.4.1 forced colours: the sorted size column is underlined')
    goto(page, base, 'tiers', 'Tiers')
    expect(page.get_by_role('button', name='Simulate', exact=True)).to_be_enabled()
    page.get_by_role('button', name='Simulate', exact=True).click()
    expect(page.get_by_role('heading', name='Alternatives')).to_be_visible()
    check(css('.alts tr.current', 'outlineStyle') != 'none' and css('.alts tbody tr:not(.current)', 'outlineStyle') == 'none', '1.4.1 forced colours: the plan on show is outlined')
    page.context.close()

    check(not errors, f'no console errors or page errors in the accessibility leg: {errors[:3]}')
