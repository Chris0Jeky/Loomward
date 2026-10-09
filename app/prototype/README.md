# Loomward interface prototype: Woven atlas + Observatory

A design reference for the real (Svelte) app. No build step, no network, synthetic data only.

```sh
py -3 -m http.server -d app/prototype 8040     # then open http://127.0.0.1:8040/
py -3 app/prototype/check.py                    # proving check (Playwright Chromium, SwiftShader)
py -3 app/prototype/check.py --gpu              # same, with the hardware rasterizer
```

Browsers refuse ES modules over `file://`, so opening `index.html` from disk shows a notice with
the command above instead of a blank page.

## The three threads

| Channel | Woven atlas | Observatory sunburst |
|---|---|---|
| Meaning (collection) | warp: vertical threads dyed by collection; unlabelled stays undyed | arc hue + fine radial threads |
| Residency (tier) | weft: metal threads, C: silver (hot), G: pewter, E: iron (cold); hollow = online-only; missing = unknown | concentric threads clipped to the arc |
| Permission (hold) | selvedge: stitched band = protected, running stitch = pinned, dotted = unknown; drawn only where a hold begins | the same stitches on the arc's outer rim |

The cloth is a 3/1 warp-faced twill (denim's structure): the meaning hue dominates and the weft
shows as diagonal flecks, so the channels cross without ever blending into one fill.

## Modules

| File | Public API |
|---|---|
| `render/woven-treemap.js` | `createWovenTreemap(canvas, opts)` -> `setRoot`, `focus`, `up`, `setBasis`, `setPalette`, `reveal`, `benchmark`, `renderFrame`; `drawSwatch` |
| `render/sunburst.js` | `createSunburst(canvas, opts)` -> `setRoot`, `focus`, `up`, `setBasis`, `setPalette`, `rotateBy`, `benchmark`, `renderFrame` |
| `render/gauges.js` | `createGauge`, `createSparkline`, `createMemoryLedger` |
| `data/synthetic.js` | `createWorkspace(seed)`, `createTelemetry(seed)`, `formatBytes` |
| `styles.css` | every colour and face is a token on `:root` (Atlas) and `[data-mode="observatory"]` |

Renderers take colours from the host (`app.js` reads the CSS tokens), never from constants.
