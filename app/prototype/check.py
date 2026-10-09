"""Proving check for the Loomward interface prototype (app/prototype).

Serves the folder on 127.0.0.1, opens it in Playwright Chromium, switches modes, drills into the
woven treemap and the sunburst, asserts no console errors and no off-host requests, measures the
treemap frame time on the full synthetic tree, and saves screenshots to evidence/v3/ui-prototype/.

    py -3 app/prototype/check.py
"""
from __future__ import annotations

import functools
import http.server
import json
import math
import sys
import threading
from pathlib import Path

from playwright.sync_api import sync_playwright

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
OUT = REPO / "evidence" / "v3" / "ui-prototype"


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def crumbs(page) -> list[str]:
    return page.locator("#crumbs li").all_inner_texts()


def wait_idle(page, ms=900):
    page.wait_for_timeout(ms)


def run() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=str(HERE)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_port}/index.html"
    results: dict = {"url_host": "127.0.0.1", "screenshots": []}
    failures: list[str] = []

    with sync_playwright() as pw:
        # --gpu asks Chromium for the hardware rasterizer (Windows: D3D11); default headless is SwiftShader
        gpu_args = ["--use-angle=d3d11", "--enable-gpu", "--ignore-gpu-blocklist", "--enable-gpu-rasterization"] if "--gpu" in sys.argv else []
        browser = pw.chromium.launch(headless=True, args=gpu_args)
        for label, viewport, dpr in (("1440", {"width": 1440, "height": 900}, 1), ("390", {"width": 390, "height": 844}, 2)):
            # bypass_csp only lets Playwright evaluate its probes; the page CSP stays in force for users
            page = browser.new_page(viewport=viewport, device_scale_factor=dpr, bypass_csp=True)
            errors: list[str] = []
            offhost: list[str] = []
            page.on("console", lambda m: errors.append(m.text) if m.type == "error" else None)
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.on("request", lambda r: offhost.append(r.url) if not r.url.startswith(("http://127.0.0.1", "data:", "blob:")) else None)
            page.goto(base)
            page.wait_for_function("window.loomward && window.loomward.ready")
            assert page.locator("#module-fallback").is_hidden(), "module fallback still visible"
            assert "Synthetic" in page.locator("#synthetic-flag").inner_text()
            wait_idle(page, 2200)  # let the loom reveal finish

            def shot(name, full=False):
                path = OUT / f"{name}-{label}.png"
                page.screenshot(path=str(path), full_page=full)
                results["screenshots"].append(str(path.relative_to(REPO)).replace("\\", "/"))

            shot("atlas")
            if label == "1440":
                shot("atlas-full", full=True)

            # --- treemap: keyboard drill, then mouse drill, then back ---
            start = crumbs(page)
            page.locator("#atlas-canvas").focus()
            page.keyboard.press("Enter")
            wait_idle(page)
            after_key = crumbs(page)
            if len(after_key) != len(start) + 1:
                failures.append(f"[{label}] keyboard drill did not open a region: {start} -> {after_key}")
            box = page.locator("#atlas-canvas").bounding_box()
            page.mouse.click(box["x"] + box["width"] * 0.3, box["y"] + box["height"] * 0.4)
            wait_idle(page)
            after_click = crumbs(page)
            if len(after_click) <= len(after_key):
                failures.append(f"[{label}] click drill did not open a region: {after_key} -> {after_click}")
            page.mouse.move(box["x"] + box["width"] * 0.55, box["y"] + box["height"] * 0.5)
            wait_idle(page, 300)
            shot("atlas-drilled")
            page.locator("#atlas-canvas").focus()
            page.keyboard.press("Escape")
            wait_idle(page)
            if len(crumbs(page)) != len(after_click) - 1:
                failures.append(f"[{label}] Escape did not go up one level")
            page.locator("#crumbs button").first.click()
            wait_idle(page)

            if label == "1440":
                # Animated drill first, on a settled page: rAF intervals while the zoom runs.
                anim = page.evaluate(
                    """async () => {
                      const lw = window.loomward, n = lw.ws.root.children[0];
                      const d = []; let last = performance.now(), go = true;
                      const tick = (t) => { d.push(t - last); last = t; if (go) requestAnimationFrame(tick); };
                      requestAnimationFrame(tick);
                      lw.atlas.focus(n);
                      await new Promise(r => setTimeout(r, 700)); go = false;
                      lw.atlas.focus(lw.ws.root, { animate: false });
                      d.sort((a, b) => a - b);
                      return { frames: d.length, median: d[d.length >> 1], p95: d[Math.floor(d.length * .95)], max: d[d.length - 1] };
                    }"""
                )
                results["renderer"] = page.evaluate("""() => { const g = document.createElement('canvas').getContext('webgl');
                    const d = g && g.getExtension('WEBGL_debug_renderer_info'); return d ? g.getParameter(d.UNMASKED_RENDERER_WEBGL) : 'unknown'; }""")
                # Frame time on the full synthetic tree (every node reachable from the focus).
                bench = page.evaluate("window.loomward.atlas.benchmark(120)")
                bench["nodes"] = page.evaluate("window.loomward.ws.nodeCount")
                # Same tree with unlimited nesting: the MIN_AREA fold is the only bound.
                deep = page.evaluate("window.loomward.atlas.benchmark(120, { maxDepth: 99 })")
                # 50,000 siblings in one folder: the case where WinDirStat-style rendering drowns.
                flat = page.evaluate(
                    """() => {
                      const lw = window.loomward, root = lw.ws.root;
                      const dir = { name: 'stress', kind: 'dir', depth: 2, parent: root.children[0], meaning: 'build', residency: 'c', permission: 'none', children: [], collections: [], unknowns: [], unmeasured: 0, files: 50000 };
                      let s = 0;
                      for (let i = 0; i < 50000; i++) { const v = Math.floor(4096 * Math.exp(((i * 7919) % 1000) / 1000 * 9)); s += v;
                        dir.children.push({ name: 'f' + i, kind: 'file', depth: 3, parent: dir, size: v, alloc: v, meaning: ['build','projects','media','models'][i % 4], residency: ['c','g','e'][i % 3], permission: 'none', children: null, collections: [], unknowns: [], unmeasured: 0, files: 1 }); }
                      dir.size = dir.alloc = s;
                      lw.atlas.focus(dir, { animate: false });
                      const r = lw.atlas.benchmark(120);
                      lw.atlas.focus(root, { animate: false });
                      return r;
                    }"""
                )
                results["treemap_benchmark_all_depths"] = deep
                results["treemap_benchmark_50k_siblings"] = flat
                for name, b in (("all depths", deep), ("50k siblings", flat)):
                    if b["p95"] > 16.7:
                        failures.append(f"treemap ({name}) p95 frame {b['p95']:.2f} ms exceeds 16.7 ms")
                results["treemap_benchmark"] = bench
                # Live: repaint in every animation frame for 2 s; CPU time per paint and frame pacing.
                live = """async (which) => {
                  const lw = window.loomward, r = which === 'atlas' ? lw.atlas : lw.orbit;
                  const cpu = [], gaps = []; let last = 0;
                  await new Promise((done) => {
                    const tick = (t) => {
                      if (last) gaps.push(t - last); last = t;
                      if (which === 'orbit') r.rotateBy(0.01);
                      const s = performance.now(); r.renderFrame(); cpu.push(performance.now() - s);
                      if (cpu.length < 120) requestAnimationFrame(tick); else done();
                    };
                    requestAnimationFrame(tick);
                  });
                  const q = (a, p) => [...a].sort((x, y) => x - y)[Math.floor(a.length * p)];
                  return { cpu_mean: cpu.reduce((a, b) => a + b, 0) / cpu.length, cpu_p95: q(cpu, .95), frame_median: q(gaps, .5), frame_p95: q(gaps, .95), dropped: gaps.filter((g) => g > 20).length, frames: gaps.length };
                }"""
                results["treemap_live"] = page.evaluate(live, "atlas")
                results["treemap_zoom_raf_ms"] = anim
                if bench["p95"] > 16.7:
                    failures.append(f"treemap p95 frame {bench['p95']:.2f} ms exceeds 16.7 ms")

            # --- observatory ---
            page.locator('.mode-switch [data-mode="observatory"]').click()
            wait_idle(page, 1200)
            assert page.evaluate("window.loomward.mode") == "observatory"
            assert page.locator("#orbit-canvas").is_visible() and page.locator("#atlas-canvas").is_hidden()
            shot("observatory")
            start = crumbs(page)
            ob = page.locator("#orbit-canvas").bounding_box()
            w, h = ob["width"], ob["height"]
            R = max(40, min(w, h) / 2 - 34)
            r0 = max(34, R * 0.24)
            T = (R - r0) / 4
            # the largest child starts at 12 o'clock and runs clockwise: aim a few degrees in
            a = -math.pi / 2 + 0.12
            px, py = ob["x"] + w / 2 + (r0 + T / 2) * math.cos(a), ob["y"] + h / 2 + (r0 + T / 2) * math.sin(a)
            page.mouse.click(px, py)
            wait_idle(page, 1100)
            zoomed = crumbs(page)
            if len(zoomed) != len(start) + 1:
                failures.append(f"[{label}] sunburst click did not zoom: {start} -> {zoomed}")
            # orbit by drag
            page.mouse.move(ob["x"] + w / 2 + R * 0.8, ob["y"] + h / 2)
            page.mouse.down()
            page.mouse.move(ob["x"] + w / 2, ob["y"] + h / 2 + R * 0.8, steps=8)
            page.mouse.up()
            wait_idle(page, 900)
            if len(crumbs(page)) != len(zoomed):
                failures.append(f"[{label}] dragging to orbit changed focus")
            page.mouse.move(px, py)
            wait_idle(page, 300)
            shot("observatory-zoomed")
            if label == "1440":
                results["sunburst_benchmark"] = page.evaluate("window.loomward.orbit.benchmark(60)")
                results["sunburst_live_orbit"] = page.evaluate(live, "orbit")
                results["sunburst_benchmark_rebuild"] = page.evaluate("window.loomward.orbit.benchmark(60, { rebuild: true })")
            page.locator('.mode-switch [data-mode="atlas"]').click()
            wait_idle(page, 600)
            assert page.evaluate("window.loomward.mode") == "atlas"

            # no horizontal page scroll at either width
            overflow = page.evaluate("document.documentElement.scrollWidth - document.documentElement.clientWidth")
            if overflow > 0:
                failures.append(f"[{label}] horizontal overflow of {overflow}px")
            if errors:
                failures.append(f"[{label}] console errors: {errors}")
            if offhost:
                failures.append(f"[{label}] off-host requests: {offhost}")
            page.close()
        # prefers-reduced-motion: no reveal, no zoom tween; a drill lands at once
        page = browser.new_page(viewport={"width": 1440, "height": 900}, reduced_motion="reduce", bypass_csp=True)
        rm_errors: list[str] = []
        page.on("pageerror", lambda e: rm_errors.append(str(e)))
        page.goto(base)
        page.wait_for_function("window.loomward && window.loomward.ready")
        before = crumbs(page)
        page.locator("#atlas-canvas").focus()
        page.keyboard.press("Enter")
        page.wait_for_timeout(60)
        if len(crumbs(page)) != len(before) + 1:
            failures.append("[reduced motion] drill did not land immediately")
        if page.evaluate("getComputedStyle(document.querySelector('.mode-thumb')).transitionDuration") not in ("0s",):
            failures.append("[reduced motion] CSS transitions still run")
        if rm_errors:
            failures.append(f"[reduced motion] errors: {rm_errors}")
        results["reduced_motion"] = "checked"
        page.close()
        browser.close()
    server.shutdown()

    results["failures"] = failures
    (OUT / ("check-results-gpu.json" if "--gpu" in sys.argv else "check-results.json")).write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    b = results.get("treemap_benchmark", {})
    if b:
        print(f"treemap: {b['nodes']} nodes, {b['cells']} drawn cells, layout {b['layoutMs']:.1f} ms, frame mean {b['mean']:.2f} ms p95 {b['p95']:.2f} ms max {b['max']:.2f} ms")
    for key, name in (("treemap_benchmark_all_depths", "all depths"), ("treemap_benchmark_50k_siblings", "50k siblings in one folder")):
        if key in results:
            d = results[key]
            print(f"treemap ({name}): {d['cells']} drawn cells, layout {d['layoutMs']:.1f} ms, frame mean {d['mean']:.2f} ms p95 {d['p95']:.2f} ms")
    if "treemap_zoom_raf_ms" in results:
        z = results["treemap_zoom_raf_ms"]
        print(f"treemap zoom: {z['frames']} rAF frames, median {z['median']:.1f} ms, p95 {z['p95']:.1f} ms")
    if "sunburst_benchmark" in results:
        s = results["sunburst_benchmark"]
        print(f"sunburst orbit: {s['arcs']} arcs, frame mean {s['mean']:.2f} ms p95 {s['p95']:.2f} ms")
        s = results["sunburst_benchmark_rebuild"]
        print(f"sunburst zoom (cloth rebuilt each frame): frame mean {s['mean']:.2f} ms p95 {s['p95']:.2f} ms")
    for s in results["screenshots"]:
        print("screenshot", s)
    for key, name in (("treemap_live", "treemap live repaint"), ("sunburst_live_orbit", "sunburst live orbit")):
        if key in results:
            d = results[key]
            print(f"{name}: cpu mean {d['cpu_mean']:.2f} ms p95 {d['cpu_p95']:.2f} ms, frame median {d['frame_median']:.1f} ms p95 {d['frame_p95']:.1f} ms, {d['dropped']}/{d['frames']} frames over 20 ms")
    if "renderer" in results:
        print("canvas renderer:", results["renderer"])
    for f in failures:
        print("FAIL", f)
    print("PASS" if not failures else "FAILED")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(run())
