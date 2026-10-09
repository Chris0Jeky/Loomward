/**
 * Loomward prototype shell: wires the synthetic workspace into the woven atlas, the
 * observatory sunburst, the inspector, the review queue, the tier simulation and the gauges.
 * Every string from data goes through textContent (h()), never innerHTML.
 */
import { createWorkspace, createTelemetry, formatBytes, MEANINGS, RESIDENCIES } from './data/synthetic.js';
import { createWovenTreemap, drawSwatch } from './render/woven-treemap.js';
import { createSunburst } from './render/sunburst.js';
import { createGauge, createSparkline, createMemoryLedger } from './render/gauges.js';

document.getElementById('module-fallback').hidden = true;

const $ = (id) => document.getElementById(id);
const reducedMQ = matchMedia('(prefers-reduced-motion: reduce)');
const reduced = () => reducedMQ.matches;
function h(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (v == null || v === false) continue;
    if (k === 'class') e.className = v;
    else if (k === 'style') e.style.cssText = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of kids.flat()) if (c != null && c !== false) e.append(c.nodeType ? c : String(c));
  return e;
}

// ---------- data ----------
const ws = createWorkspace();
(function count(n) {
  if (!n.children) { n.files = n.kind === 'file' ? 1 : 0; return n.files; }
  n.files = n.children.reduce((s, c) => s + count(c), 0);
  return n.files;
})(ws.root);
const volumeOf = (n) => { while (n && n.kind !== 'volume') n = n.parent; return n; };
const pathOf = (n) => {
  const parts = [];
  for (let p = n; p && p.kind !== 'workspace'; p = p.parent) parts.unshift(p.name);
  if (!parts.length) return n.children.map((c) => c.name).join('  ');
  return parts[0] + (parts.length > 1 ? '\\' + parts.slice(1).join('\\') : '\\');
};
let basis = 'size';
const sizeOf = (n) => (basis === 'alloc' ? n.alloc : n.size);

// ---------- palette from tokens ----------
function readPalette() {
  const cs = getComputedStyle(document.documentElement);
  const t = (k) => cs.getPropertyValue(k).trim();
  const meaning = { none: t('--meaning-none') };
  for (const k of Object.keys(MEANINGS)) meaning[k] = t(`--meaning-${k}`);
  const obs = document.documentElement.dataset.mode === 'observatory';
  return {
    ink: t('--cloth-ink'), frame: t('--cloth-frame'), head: t('--cloth-head'), scrim: t('--cloth-scrim'),
    label: t('--cloth-label'), labelDim: t('--cloth-label-dim'), fringe: t('--cloth-fringe'), loose: t('--cloth-loose'),
    cursor: t('--cloth-cursor'), hover: t('--cloth-hover'), shuttle: t('--cloth-shuttle'),
    track: t('--cloth-track'), trackFill: t('--cloth-track-fill'), scale: t('--cloth-scale'), hole: t('--cloth-frame'),
    meaning, residency: { c: t('--residency-c'), g: t('--residency-g'), e: t('--residency-e'), cloud: t('--residency-cloud') },
    permission: t('--permission'), unknown: t('--unknown'),
    fontDisplay: `400 21px ${t('--font-display')}`,
    fontLabel: `600 11.5px ${t('--font-ui')}`,
    fontMono: obs ? `400 10.5px ${t('--font-mono')}` : `500 11px ${t('--font-ui')}`,
    fontHead: obs ? `600 12.5px ${t('--font-mono')}` : `400 16px ${t('--font-display')}`,
    fontMonoSmall: `400 10px ${t('--font-mono')}`,
    fontCentre: obs ? `600 15px ${t('--font-ui-display')}` : `400 19px ${t('--font-display')}`,
  };
}
let palette = readPalette();
const fmt = (n) => formatBytes(n);

// ---------- map renderers ----------
const atlasCanvas = $('atlas-canvas'), orbitCanvas = $('orbit-canvas');
const tip = $('tip');
let mode = 'atlas';
let selected = null;

const handlers = {
  palette, format: fmt, reducedMotion: reduced,
  onHover(node, cell, ev, fromKeys) {
    if (cell?.agg) { showTip(ev, `${cell.agg.count.toLocaleString('en-GB')} smaller items`, `${fmt(cell.agg.size)} · too small to draw here`); inspectAgg(cell.agg); return; }
    if (!node) { hideTip(); inspect(selected || current().focused, selected ? 'Selected' : 'Current view'); return; }
    if (ev) showTip(ev, node.name, `${fmt(sizeOf(node))}${node.unmeasured ? ` · ${node.unmeasured} unmeasured` : ''}`); else hideTip();
    inspect(node, 'Hover preview');
    if (fromKeys) $('map-live').textContent = `${node.name}, ${fmt(sizeOf(node))}. ${node.children?.length ? 'Enter opens it.' : ''}`;
  },
  onSelect(node, cell) {
    if (cell?.agg) { inspectAgg(cell.agg); return; }
    selected = node;
    if (node) inspect(node, 'Selected');
  },
  onFocus(node) {
    crumbs(node);
    if (!selected || !isWithin(selected, node)) selected = null;
    inspect(selected || node, selected ? 'Selected' : 'Current view');
    const kids = (node.children || []).filter((c) => sizeOf(c) > 0).length;
    $('map-live').textContent = `Opened ${pathOf(node)}, ${fmt(sizeOf(node))}, ${kids} regions.`;
  },
};
const isWithin = (n, anc) => { for (let p = n; p; p = p.parent) if (p === anc) return true; return false; };
const atlas = createWovenTreemap(atlasCanvas, { ...handlers, maxDepth: 3 });
const orbit = createSunburst(orbitCanvas, { ...handlers, rings: 4 });
const current = () => (mode === 'atlas' ? atlas : orbit);

function showTip(ev, title, sub) {
  if (!ev) return;
  const well = tip.parentElement.getBoundingClientRect();
  tip.replaceChildren(h('b', {}, title), h('span', {}, sub));
  tip.hidden = false;
  const x = ev.clientX - well.left, y = ev.clientY - well.top;
  const tw = tip.offsetWidth, th = tip.offsetHeight;
  tip.style.left = `${Math.min(well.width - tw - 8, x + 16)}px`;
  tip.style.top = `${y + th + 24 > well.height ? y - th - 12 : y + 18}px`;
}
const hideTip = () => { tip.hidden = true; };

function crumbs(node) {
  const chain = [];
  for (let p = node; p; p = p.parent) chain.unshift(p);
  $('crumbs').replaceChildren(...chain.map((n, i) => h('li', {},
    h('button', { type: 'button', 'aria-current': i === chain.length - 1 ? 'location' : null, onclick: () => i < chain.length - 1 && current().focus(n) },
      n.kind === 'workspace' ? 'All volumes' : n.name))));
}

// ---------- inspector ----------
const describeResidency = (n) => {
  if (n.kind === 'workspace') return ['Three volumes', 'C: hot NVMe, G: warm NVMe, E: cold HDD'];
  if (n.kind === 'gap') return ['Unknown', 'Counted by the volume, not attributed to any file'];
  if (n.residency === 'cloud') return [RESIDENCIES.cloud, 'Logical size is counted; 0 B is resident on a local disk'];
  if (n.residency == null) return ['Unknown', 'No residency evidence'];
  const v = volumeOf(n);
  return [RESIDENCIES[n.residency], v ? `${v.media}, observed ${v.observedMin} min ago (synthetic)` : 'No volume record'];
};
function permissionOrigin(n) {
  let o = n;
  while (o.parent && o.parent.permission === n.permission && o.parent.kind !== 'workspace') o = o.parent;
  return o;
}
const describePermission = (n) => {
  if (n.permission == null) return ['Unknown', n.kind === 'gap' ? 'No permission evidence for unattributed bytes' : 'Access denied without elevation; nothing assumed'];
  const o = permissionOrigin(n);
  const from = o !== n ? `Inherited from ${pathOf(o)}` : null;
  if (n.permission === 'protected') return ['Protected', from || 'Hold begins here: system or backup material'];
  if (n.permission === 'pinned') return ['Pinned', from || `Hold begins here${n.collections.length ? `: ${n.collections.join(', ')}` : ''}`];
  return ['No hold', 'Not pinned or protected. That is not permission to change it.'];
};
const describeMeaning = (n) => {
  if (n.kind === 'gap') return ['Unknown', 'Contents not attributed'];
  if (n.kind === 'workspace' || n.kind === 'volume') return ['Mixed', 'Open it to see meanings by region'];
  if (n.meaning == null) return ['Unlabelled', 'No collection, and no meaning holds most of its bytes'];
  return [MEANINGS[n.meaning], n.inferredMeaning ? `Inferred: most bytes below here are ${MEANINGS[n.meaning]}` : 'Assigned collection (synthetic)'];
};

function inspect(n, state) {
  if (!n) return;
  const sw = h('canvas', { class: 'insp-swatch', width: 56, height: 56, 'aria-hidden': 'true' });
  const v = volumeOf(n);
  const ref = n.parent && n.kind !== 'volume' ? n.parent : null;
  const share = ref && sizeOf(ref) ? ` · ${(100 * (sizeOf(n) || 0) / sizeOf(ref)).toFixed(1)}% of ${ref.name}` : '';
  const kind = { workspace: 'Workspace', volume: 'Volume', dir: 'Folder', file: 'File', gap: 'Unattributed bytes' }[n.kind];
  const items = n.children ? ` · ${n.files.toLocaleString('en-GB')} files` : '';
  const max = Math.max(n.size || 0, n.alloc || 0) || 1;
  const sizeRow = (label, val) => h('div', { class: 'size-row' }, h('span', {}, label),
    h('div', { class: 'size-bar' }, h('span', { style: `transform:scaleX(${val == null ? 0 : (val / max).toFixed(4)})` })),
    h('b', {}, fmt(val)));
  let note = null;
  if (n.size != null && n.alloc != null && n.size > 0) {
    const r = n.alloc / n.size;
    if (n.residency === 'cloud') note = 'Online-only: logical bytes, none resident.';
    else if (r < 0.9) note = `Allocated is ${Math.round(r * 100)}% of logical (sparse, compressed or tiny files).`;
  }
  if (n.size == null) note = 'Size unmeasured. It is shown as unknown, never as zero.';
  const [mm, ms] = describeMeaning(n), [rm, rs] = describeResidency(n), [pm, ps] = describePermission(n);
  const unknowns = [...n.unknowns];
  if (n.kind !== 'workspace') unknowns.push('Last access time not recorded: "no recent observations" is not "unused"');
  if (v && v.coverage < 1) unknowns.push(`${v.name} scan paused at ${Math.round(v.coverage * 100)}%`);
  const meaningColor = palette.meaning[n.meaning] || palette.meaning.none;
  $('inspector').replaceChildren(
    h('div', { class: 'insp-state' }, h('span', {}, 'Inspector'), h('span', { class: 'chip' }, state)),
    h('div', { class: 'insp-head' }, sw, h('div', {}, h('h2', { id: 'insp-name' }, n.kind === 'workspace' ? 'All volumes' : n.name), h('p', { class: 'insp-path' }, pathOf(n)))),
    h('p', { class: 'insp-kind' }, `${kind}${items}${share}`),
    h('div', { class: 'sizes' }, sizeRow('Logical', n.size), sizeRow('Allocated', n.alloc), note && h('p', { class: 'size-note' }, note)),
    h('dl', { class: 'channels' },
      h('div', { class: 'channel' }, h('dt', {}, 'Warp', h('small', {}, 'meaning')), h('dd', {},
        h('span', {}, h('i', { class: 'thread-dot', style: `background:${meaningColor}` }), mm), h('span', { class: 'sub' }, ms),
        n.collections.length ? h('span', { class: 'tagrow' }, n.collections.map((c) => h('span', { class: 'tag' }, c))) : null)),
      h('div', { class: 'channel' }, h('dt', {}, 'Weft', h('small', {}, 'residency')), h('dd', {}, h('span', {}, rm), h('span', { class: 'sub' }, rs))),
      h('div', { class: 'channel' }, h('dt', {}, 'Selvedge', h('small', {}, 'permission')), h('dd', {}, h('span', {}, pm), h('span', { class: 'sub' }, ps))),
      h('div', { class: 'channel' }, h('dt', {}, 'Unknown', h('small', {}, 'kept visible')), h('dd', {}, h('ul', { class: 'unknown-list' }, unknowns.map((u) => h('li', {}, u))))),
    ),
    h('p', { class: 'insp-foot' }, 'Describes only. No move, delete or clean-up exists in this build.'),
  );
  requestAnimationFrame(() => drawSwatch(sw, palette, { meaning: n.kind === 'gap' ? null : n.meaning, residency: n.kind === 'gap' ? null : n.residency, permission: n.permission }));
}
function inspectAgg(a) {
  $('inspector').replaceChildren(
    h('div', { class: 'insp-state' }, h('span', {}, 'Inspector'), h('span', { class: 'chip' }, 'Folded threads')),
    h('h2', { id: 'insp-name' }, `${a.count.toLocaleString('en-GB')} smaller items`),
    h('p', { class: 'insp-path' }, pathOf(a.parent)),
    h('p', { class: 'insp-kind' }, `${fmt(a.size)} together, each too small to draw at this scale.`),
    h('p', { class: 'insp-foot' }, `Open ${a.parent.name} to see them as their own cells. Folding bounds the render; it hides nothing from the totals.`),
  );
}

// ---------- coverage header ----------
function renderCoverage() {
  $('volumes').replaceChildren(...ws.volumes.map((v) => {
    const scanned = v.size - (v.children.find((c) => c.kind === 'gap')?.size || 0);
    const gap = v.used - scanned;
    const state = v.coverage < 1 ? `Paused at ${Math.round(v.coverage * 100)}% · ${v.observedMin} min ago` : `Complete · ${v.observedMin} min ago`;
    return h('li', { class: 'vol', style: `--weft: var(--residency-${v.residency})` },
      h('div', { class: 'vol-top' }, h('span', { class: 'vol-letter' }, v.name), h('span', { class: 'vol-name' }, v.label.split(' · ')[1] + ' · ' + v.media),
        h('span', { class: `vol-state${v.coverage < 1 ? ' partial' : ''}` }, state)),
      h('div', { class: 'vol-bar', role: 'img', 'aria-label': `${v.name}: ${fmt(scanned)} attributed, ${fmt(gap)} not attributed, ${fmt(v.capacity - v.used)} free of ${fmt(v.capacity)}` },
        h('span', { class: 'scanned', style: `width:${(100 * scanned / v.capacity).toFixed(2)}%` }),
        h('span', { class: 'unscanned', style: `width:${(100 * gap / v.capacity).toFixed(2)}%` })),
      h('div', { class: 'vol-figs' }, h('span', {}, h('b', {}, fmt(v.used)), ` used of ${fmt(v.capacity)}`), h('span', {}, `${fmt(gap)} ${v.coverage < 1 ? 'unscanned' : 'unattributed'}`)));
  }));
  let denied = 0, unm = 0;
  for (const n of ws.byId.values()) if (n.denied) denied++;
  for (const v of ws.volumes) unm += v.unmeasured;
  $('unknowns').replaceChildren('Unknown, not zero: ', h('b', {}, `${denied} folders`), ' denied access, ', h('b', {}, `${unm} items`),
    ' unmeasured, ', h('b', {}, fmt(ws.volumes.reduce((s, v) => s + (v.children.find((c) => c.kind === 'gap')?.size || 0), 0))),
    ` not attributed. ${ws.nodeCount.toLocaleString('en-GB')} synthetic nodes.`);
}

// ---------- thread legend ----------
function renderThreads() {
  const sw = (opts, cls) => { const c = h('canvas', { 'aria-hidden': 'true' }); requestAnimationFrame(() => drawSwatch(c, palette, opts)); return c; };
  const item = (opts, label, cls) => h('li', { class: cls }, sw(opts), label);
  $('threads').replaceChildren(
    h('div', { class: 'tg' }, h('h3', {}, h('em', {}, 'Warp'), 'carries meaning'),
      h('p', {}, 'The vertical threads are dyed by collection. Unlabelled cloth stays undyed.'),
      h('ul', {}, [...Object.entries(MEANINGS).map(([k, l]) => item({ meaning: k, only: 'warp' }, l)), item({ meaning: null, only: 'warp' }, 'Unlabelled')])),
    h('div', { class: 'tg' }, h('h3', {}, h('em', {}, 'Weft'), 'carries residency'),
      h('p', {}, 'The crossing threads are metals by tier: hot is bright, cold is dark.'),
      h('ul', {}, item({ residency: 'c', only: 'weft' }, 'C: silver, hot'), item({ residency: 'g', only: 'weft' }, 'G: pewter, warm'), item({ residency: 'e', only: 'weft' }, 'E: iron, cold'),
        item({ residency: 'cloud', only: 'weft' }, 'Hollow: online-only'), item({ residency: null, only: 'weft' }, 'Missing: unknown'))),
    h('div', { class: 'tg' }, h('h3', {}, h('em', {}, 'Selvedge'), 'carries permission'),
      h('p', {}, 'The edge is stitched where a hold begins; inside it, children inherit.'),
      h('ul', {}, item({ permission: 'protected', only: 'selvedge' }, 'Protected: stitched band'), item({ permission: 'pinned', only: 'selvedge' }, 'Pinned: running stitch'),
        item({ permission: null, only: 'selvedge' }, 'Unknown: dotted, e.g. denied'))),
  );
}

// ---------- review queue ----------
const decisions = new Map();
function renderReview() {
  const abst = ws.review.filter((r) => !r.label).length;
  $('review-count').textContent = `${ws.review.length - abst} proposals · ${abst} abstentions · session only`;
  $('review-list').replaceChildren(...ws.review.map((r, i) => {
    const d = decisions.get(i);
    const prop = r.label
      ? h('div', { class: 'rq-prop' }, h('i', { class: 'thread-dot', style: `background:${palette.meaning[r.label]}` }), MEANINGS[r.label],
        h('span', { class: 'rq-meter', 'aria-hidden': 'true' }, h('span', { style: `width:${Math.round(r.score * 100)}%` })),
        h('span', { class: 'score' }, `${r.score.toFixed(2)} rel.`))
      : h('div', { class: 'rq-prop' }, h('span', { class: 'rq-abstain' }, 'Abstains'));
    const show = h('button', { class: 'btn quiet', type: 'button', onclick: () => reveal(r.node) }, 'Show in map');
    let actions;
    if (d) actions = h('div', { class: 'rq-actions', role: 'status' }, d, ' ', h('button', { class: 'btn', type: 'button', onclick: () => { decisions.delete(i); renderReview(); } }, 'Undo'));
    else {
      const sel = h('select', { 'aria-label': `Choose a label for ${r.node.name}` },
        h('option', { value: '' }, 'Other label…'), ...Object.entries(MEANINGS).map(([k, l]) => h('option', { value: k }, l)));
      sel.addEventListener('change', () => { if (sel.value) { decisions.set(i, `Saved "${MEANINGS[sel.value]}" for this session. Nothing moved.`); renderReview(); } });
      actions = h('div', { class: 'rq-actions' },
        r.label ? h('button', { class: 'btn primary', type: 'button', onclick: () => { decisions.set(i, `Saved "${MEANINGS[r.label]}" for this session. Nothing moved.`); renderReview(); } }, `Accept ${MEANINGS[r.label]}`) : null,
        sel, h('button', { class: 'btn quiet', type: 'button', onclick: () => { decisions.set(i, 'Deferred. Not asked again this scan.'); renderReview(); } }, 'Not now'), show);
    }
    return h('li', { class: `rq${d ? ' done' : ''}` },
      h('div', { class: 'rq-file' }, h('p', { class: 'rq-name', title: pathOf(r.node) }, r.node.name)), prop,
      h('p', { class: 'rq-why' }, r.why), actions);
  }));
}
function reveal(n) {
  const target = n.children?.length ? n.parent : n.parent;
  current().focus(target || ws.root);
  selected = n;
  atlas.select(n);
  inspect(n, 'Selected');
  $('stage').scrollIntoView({ behavior: reduced() ? 'auto' : 'smooth', block: 'start' });
}

// ---------- tier simulation ----------
let keepCurrent = false;
function renderTiers() {
  const plan = ws.tierPlan;
  const delta = { 'C:': 0, 'G:': 0, 'E:': 0 };
  if (!keepCurrent) for (const m of plan.moves) { delta[m.from] -= m.bytes; delta[m.to] += m.bytes; }
  const vols = ws.volumes.map((v) => {
    const after = v.used + delta[v.name];
    const res = 1 - plan.reservePct / 100;
    return h('div', { class: 'tier-vol', style: `--weft: var(--residency-${v.residency})` },
      h('span', { class: 'l' }, v.name),
      h('div', { class: 'tier-track', role: 'img', 'aria-label': `${v.name} now ${fmt(v.used)}, simulated ${fmt(after)} of ${fmt(v.capacity)}` },
        h('span', { class: 'now', style: `transform:scaleX(${(v.used / v.capacity).toFixed(4)})` }),
        h('span', { class: 'after', style: `transform:scaleX(${(after / v.capacity).toFixed(4)})` }),
        h('span', { class: 'reserve', style: `left:${(100 * res).toFixed(1)}%`, title: `${plan.reservePct}% reserve` })),
      h('span', { class: 'tier-figs' }, h('b', {}, `${Math.round(100 * after / v.capacity)}%`), `${fmt(v.capacity - after)} free`));
  });
  $('tier-body').replaceChildren(
    h('p', { class: 'tier-goal' }, plan.goal + '.'),
    h('p', { class: 'tier-key', 'aria-hidden': 'true' }, h('span', {}, h('i', { class: 'k-now' }), 'now'), h('span', {}, h('i', { class: 'k-after' }), keepCurrent ? 'keep current' : 'after the simulated plan'), h('span', {}, h('i', { class: 'k-res' }), `${plan.reservePct}% reserve`)),
    ...vols,
    h('ul', { class: 'tier-moves', style: keepCurrent ? 'opacity:.45' : '' }, plan.moves.map((m) => h('li', {},
      h('span', { class: 'what' }, m.node.name), h('span', { class: 'route' }, `${fmt(m.bytes)}  ${m.from} → ${m.to}`),
      h('span', { class: 'why' }, m.reason + '.'), h('span', { class: 'caveat' }, m.caveat + '.')))),
    h('p', { class: 'tier-kept' }, 'Left in place: ' + plan.kept.join('; ') + '.'),
    h('div', { class: 'tier-actions' },
      h('button', { class: 'btn', type: 'button', 'aria-pressed': String(keepCurrent), onclick: () => { keepCurrent = !keepCurrent; renderTiers(); } }, keepCurrent ? 'Show the simulated plan' : 'Compare: keep everything where it is'),
      h('button', { class: 'btn', type: 'button', onclick: () => reveal(plan.moves[0].node) }, 'Show first group in map')),
  );
}

// ---------- resources ----------
const tele = createTelemetry();
const g = {};
function mountGauges() {
  const box = $('gauges');
  const make = (label, opts) => { const d = h('div'); box.append(d); return createGauge(d, { label, ...opts }); };
  g.cpu = make('CPU', { unit: '%' });
  g.gpu = make('GPU', { unit: '%' });
  g.vram = make('VRAM', { unit: 'GiB', max: 24, format: (v) => v.toFixed(1) });
  g.io = make('Disk I/O', { unit: 'MB/s', max: 3000, format: (v) => (v >= 1000 ? (v / 1000).toFixed(1) + 'k' : v.toFixed(0)) });
}
mountGauges();
const sparks = {};
$('io').replaceChildren(...['c', 'g', 'e'].map((k) => {
  const c = h('canvas', { 'aria-hidden': 'true', style: `--spark-line: var(--residency-${k}); --spark-fill: color-mix(in srgb, var(--residency-${k}) 22%, transparent)` });
  sparks[k] = { spark: createSparkline(c, { autoMax: true, length: 90 }), val: h('span', { class: 'v' }) };
  return h('div', { class: 'io-row' }, h('span', { class: 'l' }, k.toUpperCase() + ':'), c, sparks[k].val);
}));
const memory = createMemoryLedger($('memory'), { format: (n) => formatBytes(n, 1) });

function applySample(s) {
  g.cpu.update(s.cpu, '16 logical');
  g.gpu.update(s.gpu, s.episode === 'synthetic render' ? 'render load' : 'light');
  g.vram.update(s.vram, `of ${s.vramTotal} GiB`);
  const total = ['c', 'g', 'e'].reduce((a, k) => a + s.io[k].r + s.io[k].w, 0);
  g.io.update(total, 'read + write');
  for (const k of ['c', 'g', 'e']) {
    sparks[k].spark.push(s.io[k].r + s.io[k].w);
    sparks[k].val.replaceChildren('r ', h('b', {}, s.io[k].r.toFixed(0)), ' w ', h('b', {}, s.io[k].w.toFixed(0)), ' MB/s');
  }
  memory.update(s.ram);
  $('episode').textContent = `Synthetic telemetry · ${s.episode}`;
}
for (let i = 0; i < 89; i++) { const s = tele.next(); for (const k of ['c', 'g', 'e']) sparks[k].spark.push(s.io[k].r + s.io[k].w); }
applySample(tele.next());
setInterval(() => { if (!document.hidden) applySample(tele.next()); }, 1000);

// ---------- mode & basis ----------
const help = {
  atlas: ['Click a region to open it', 'Arrows move', 'Enter opens', 'Esc goes up'],
  observatory: ['Drag to orbit', 'Click an arc to zoom', 'Centre zooms out', '[ ] rotate'],
};
function renderHelp() {
  const [a, ...keys] = help[mode];
  const kbdize = (s) => { const m = s.match(/^(Arrows|Enter|Esc|\[ \])(.*)$/); return m ? [h('kbd', {}, m[1]), m[2]] : [s]; };
  $('map-help').replaceChildren(a, ...keys.flatMap((k) => [' · ', ...kbdize(k)]));
}
function setMode(next, { transition = true } = {}) {
  if (next === mode) return;
  const prevFocus = current().focused;
  const apply = () => {
    mode = next;
    document.documentElement.dataset.mode = mode;
    document.querySelectorAll('.mode-switch button').forEach((b) => b.setAttribute('aria-checked', String(b.dataset.mode === mode)));
    atlasCanvas.hidden = mode !== 'atlas';
    orbitCanvas.hidden = mode !== 'observatory';
    palette = readPalette();
    atlas.setPalette(palette); orbit.setPalette(palette);
    if (prevFocus) current().focus(prevFocus, { animate: false });
    if (mode === 'atlas') { atlas.resize(); atlas.reveal(); } else orbit.resize();
    renderThreads(); renderReview(); renderHelp();
    for (const k in sparks) sparks[k].spark.redraw();
    inspect(selected || current().focused, selected ? 'Selected' : 'Current view');
    crumbs(current().focused);
  };
  if (transition && document.startViewTransition && !reduced()) document.startViewTransition(apply);
  else apply();
}
document.querySelectorAll('.mode-switch button').forEach((b) => b.addEventListener('click', () => setMode(b.dataset.mode)));
document.querySelector('.mode-switch').addEventListener('keydown', (e) => {
  if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') { setMode(mode === 'atlas' ? 'observatory' : 'atlas'); document.querySelector(`.mode-switch [data-mode="${mode}"]`).focus(); e.preventDefault(); }
});
document.querySelectorAll('.basis button').forEach((b) => b.addEventListener('click', () => {
  basis = b.dataset.basis;
  document.querySelectorAll('.basis button').forEach((x) => x.setAttribute('aria-checked', String(x === b)));
  atlas.setBasis(basis); orbit.setBasis(basis);
  inspect(selected || current().focused, selected ? 'Selected' : 'Current view');
}));

atlas.setRoot(ws.root);
orbit.setRoot(ws.root);
renderCoverage(); renderThreads(); renderReview(); renderTiers(); renderHelp();
crumbs(ws.root); inspect(ws.root, 'Current view');
atlas.reveal();

// handle for the proving check and for poking at it from devtools
window.loomward = { ws, atlas, orbit, setMode, get mode() { return mode; }, ready: true };
