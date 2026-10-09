/**
 * Loomward prototype: deterministic synthetic workspace + telemetry.
 *
 * SYNTHETIC. Every name, size and number here is invented by a seeded generator.
 * Nothing is read from the machine that runs the page.
 *
 * Public API
 *   createWorkspace(seed?)   -> Workspace   (same seed => byte-identical tree)
 *   createTelemetry(seed?)   -> { next(): Sample }   (moving synthetic resource stream)
 *   formatBytes(n), formatCount(n)
 *
 * Node shape (one object per volume, folder, file or gap):
 *   id, name, kind: 'workspace'|'volume'|'dir'|'file'|'gap'
 *   size     logical bytes, or null when unmeasured (never 0 for "unknown")
 *   alloc    allocated bytes on disk, or null when unmeasured
 *   meaning  collection/category key (warp), or null = unlabelled
 *   residency 'c'|'g'|'e' (volume tier), 'cloud' (online-only placeholder) or null = unknown (weft)
 *   permission 'protected'|'pinned'|'none' or null = unknown, e.g. access denied (selvedge)
 *   unmeasured  count of descendants whose size is unknown
 *   unknowns    human-readable list of what this node does not know
 *   children, parent, depth, collections, modifiedDays, accessed (always null: last-access not recorded)
 */

const GiB = 2 ** 30, MiB = 2 ** 20, KiB = 1024, TiB = 2 ** 40;

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export const MEANINGS = {
  projects: 'Projects', media: 'Media', models: 'Models', build: 'Build caches',
  archives: 'Archives', documents: 'Documents', system: 'System', downloads: 'Downloads',
};
export const RESIDENCIES = { c: 'C: NVMe · hot', g: 'G: NVMe · warm', e: 'E: HDD · cold', cloud: 'Online-only placeholder' };
export const PERMISSIONS = { protected: 'Protected', pinned: 'Pinned', none: 'No hold' };

export function createWorkspace(seed = 0x10057) {
  const rnd = mulberry32(seed);
  const pick = (arr) => arr[Math.floor(rnd() * arr.length)];
  // log-normal-ish size around a median, clamped
  const lsize = (median, spread = 1.2) => {
    const g = (rnd() + rnd() + rnd() - 1.5) * 2 * spread;
    return Math.max(1, Math.round(median * Math.exp(g)));
  };
  let nextId = 0;
  const all = [];

  function node(parent, name, kind, props = {}) {
    const n = {
      id: 'n' + (nextId++), name, kind, parent,
      depth: parent ? parent.depth + 1 : 0,
      size: null, alloc: null,
      meaning: props.meaning !== undefined ? props.meaning : parent?.meaning ?? null,
      residency: props.residency !== undefined ? props.residency : parent?.residency ?? null,
      permission: props.permission !== undefined ? props.permission : parent?.permission ?? 'none',
      collections: props.collections || [],
      unknowns: props.unknowns ? [...props.unknowns] : [],
      modifiedDays: props.modifiedDays ?? Math.round(rnd() * 900),
      accessed: null,
      unmeasured: 0,
      ext: props.ext,
      children: kind === 'file' || kind === 'gap' ? null : [],
    };
    if (parent) parent.children.push(n);
    all.push(n);
    return n;
  }
  const dir = (p, name, props) => node(p, name, 'dir', props);

  function file(p, name, size, props = {}) {
    const f = node(p, name, 'file', props);
    f.size = size;
    if (size == null) f.alloc = null;
    else if (props.alloc !== undefined) f.alloc = props.alloc;
    else if (f.residency === 'cloud') f.alloc = 0; // placeholder: logical size, nothing resident
    else if (size < 700) f.alloc = 0; // resident in the MFT record
    else f.alloc = Math.ceil(size / 4096) * 4096;
    return f;
  }

  function files(p, count, stem, exts, median, props = {}) {
    for (let i = 0; i < count; i++) {
      const ext = pick(exts);
      file(p, `${stem}${String(i + 1).padStart(4, '0')}.${ext}`, lsize(median, props.spread ?? 1.1), { ext, ...props });
    }
  }

  const deniedDir = (p, name, why) => {
    const d = dir(p, name, { permission: null, meaning: null, unknowns: [why] });
    d.denied = true;
    return d;
  };

  // A source repository: src, docs, tests, .git and regenerable outputs.
  function repo(p, name, scale, props = {}) {
    const r = dir(p, name, { meaning: 'projects', ...props });
    files(dir(r, 'src'), Math.round(140 * scale), 'module_', ['rs', 'ts', 'py', 'svelte'], 9 * KiB);
    files(dir(r, 'docs'), Math.round(18 * scale), 'note_', ['md'], 6 * KiB);
    files(dir(r, 'tests'), Math.round(60 * scale), 'test_', ['rs', 'ts', 'py'], 5 * KiB);
    files(dir(r, '.git', { permission: 'protected' }), Math.round(40 * scale), 'pack_', ['pack', 'idx'], 6 * MiB, { spread: 1.6 });
    const nm = dir(r, 'node_modules', { meaning: 'build', permission: 'none', collections: ['Regenerable'] });
    for (let k = 0; k < Math.round(24 * scale); k++) {
      files(dir(nm, `pkg-${k.toString(36)}${Math.floor(rnd() * 99)}`), 6 + Math.floor(rnd() * 50), 'f', ['js', 'cjs', 'map', 'json', 'd.ts'], 7 * KiB, { spread: 1.4 });
    }
    const tg = dir(r, 'target', { meaning: 'build', permission: 'none', collections: ['Regenerable'] });
    files(dir(tg, 'debug'), Math.round(120 * scale), 'artifact_', ['rlib', 'rmeta', 'pdb', 'exe', 'o'], 3 * MiB, { spread: 1.5 });
    files(dir(tg, 'release'), Math.round(40 * scale), 'artifact_', ['rlib', 'exe', 'o'], 5 * MiB, { spread: 1.4 });
    return r;
  }

  const root = node(null, 'Workspace', 'workspace', { meaning: null, residency: null, permission: 'none' });

  // ---------------- C: NVMe 1.86 TB ----------------
  const C = node(root, 'C:', 'volume', { residency: 'c', meaning: null, permission: 'none' });
  Object.assign(C, { label: 'C: · System NVMe', media: 'NVMe SSD', capacity: 1.86 * TiB, used: 1.52 * TiB, coverage: 1, observedMin: 4, scanState: 'complete' });
  {
    const win = dir(C, 'Windows', { meaning: 'system', permission: 'protected' });
    files(dir(win, 'WinSxS'), 5200, 'component_', ['dll', 'manifest', 'mum', 'cat'], 1.6 * MiB, { spread: 1.5 });
    files(dir(win, 'System32'), 3400, 'sys_', ['dll', 'exe', 'sys', 'mui'], 900 * KiB, { spread: 1.5 });
    files(dir(win, 'Installer'), 380, 'pkg_', ['msi', 'msp'], 48 * MiB, { spread: 1.2 });
    files(dir(win, 'Fonts'), 320, 'face_', ['ttf', 'otf'], 400 * KiB);
    const pf = dir(C, 'Program Files', { meaning: 'system', permission: 'protected' });
    for (const app of ['Editor Suite', 'Browser', 'Toolchain', 'Graphics Driver', 'Media Studio', 'Office Apps', 'Sync Client']) {
      files(dir(pf, app), 120 + Math.floor(rnd() * 380), 'bin_', ['dll', 'exe', 'pak', 'dat'], 3 * MiB, { spread: 1.6 });
    }
    const pd = dir(C, 'ProgramData', { meaning: 'system', permission: 'none' });
    files(dir(pd, 'Package Cache'), 260, 'cache_', ['msi', 'cab'], 22 * MiB, { meaning: 'build' });
    deniedDir(pd, 'Protected Store', 'Access denied without elevation: size and contents unmeasured');
    file(C, 'pagefile.sys', 24 * GiB, { meaning: 'system', permission: 'protected', unknowns: ['Size from the volume record; contents are not file data'] });
    file(C, 'hiberfil.sys', 12.8 * GiB, { meaning: 'system', permission: 'protected' });
    deniedDir(C, 'System Volume Information', 'Access denied without elevation: restore points and shadow copies unmeasured');

    const user = dir(C, 'Users', { meaning: null });
    const me = dir(user, 'synthetic-user', { meaning: null });
    const app = dir(me, 'AppData', { meaning: 'build' });
    const local = dir(app, 'Local', { meaning: 'build' });
    for (const c of ['npm-cache', 'pip', 'cargo-registry', 'NuGet', 'Temp', 'Browser Cache', 'Shader Cache']) {
      const d = dir(local, c, { collections: ['Regenerable'] });
      for (let k = 0; k < 12; k++) files(dir(d, `${k.toString(16)}${k}`), 40 + Math.floor(rnd() * 120), 'blob_', ['bin', 'tmp', 'tgz', 'whl'], 260 * KiB, { spread: 1.6 });
    }
    deniedDir(local, 'Packages', 'Some app containers deny listing: 1 of 3 subtrees unmeasured');
    const ollama = dir(me, '.models', { meaning: 'models', permission: 'pinned', collections: ['Coding session'] });
    file(ollama, 'coder-14b-q5.gguf', 10.3 * GiB, { ext: 'gguf' });
    file(ollama, 'embed-small-f16.gguf', 0.6 * GiB, { ext: 'gguf' });
    file(ollama, 'vision-7b-q4.gguf', 4.8 * GiB, { ext: 'gguf', permission: 'none' });

    const docs = dir(me, 'Documents', { meaning: 'documents' });
    const inv = dir(docs, 'Invoices', { collections: ['Taxes 2025'] });
    files(inv, 140, 'invoice_', ['pdf'], 220 * KiB);
    files(dir(docs, 'Papers'), 210, 'paper_', ['pdf', 'epub'], 2.4 * MiB);
    files(dir(docs, 'Notes'), 640, 'note_', ['md', 'txt'], 8 * KiB);
    files(dir(docs, 'Contracts', { permission: 'pinned', collections: ['Client · Harbor'] }), 36, 'contract_', ['pdf', 'docx'], 640 * KiB);
    const od = dir(me, 'OneDrive', { meaning: 'documents', residency: 'cloud' });
    files(dir(od, 'Shared Albums', { meaning: 'media' }), 420, 'shared_', ['jpg', 'heic'], 3.1 * MiB);
    files(dir(od, 'Team Docs'), 180, 'team_', ['docx', 'xlsx', 'pptx'], 1.2 * MiB);
    files(dir(od, 'Pinned offline', { residency: 'c', permission: 'pinned' }), 60, 'offline_', ['docx', 'pdf'], 900 * KiB);

    const dl = dir(me, 'Downloads', { meaning: 'downloads' });
    files(dl, 60, 'installer_', ['exe', 'msi'], 140 * MiB, { meaning: 'downloads' });
    files(dl, 110, 'download_', ['zip', '7z'], 260 * MiB, { meaning: null, spread: 1.6 });
    files(dl, 170, 'document_', ['pdf'], 1.5 * MiB, { meaning: null });
    const pics = dir(me, 'Pictures', { meaning: 'media' });
    files(dir(pics, 'Screenshots'), 900, 'shot_', ['png'], 900 * KiB);
    files(dir(pics, 'Camera Roll'), 1200, 'IMG_', ['jpg', 'heic'], 3.6 * MiB);
    const vids = dir(me, 'Videos', { meaning: 'media' });
    files(dir(vids, 'Captures'), 140, 'capture_', ['mp4'], 900 * MiB, { spread: 1.3 });
    files(dir(vids, '2022 exports', { collections: ['Cold candidates'] }), 46, 'export_', ['mov', 'mp4'], 3.8 * GiB, { modifiedDays: 1100 });
    const src = dir(me, 'source', { meaning: 'projects' });
    for (const [n, s] of [['harbor-api', 1.2], ['atlas-site', 0.9], ['tessera', 1.5], ['loom-sketches', 0.5], ['render-farm-2023', 0.8]]) {
      const r = repo(src, n, s, n === 'render-farm-2023' ? { modifiedDays: 780, collections: ['Cold candidates'] } : {});
      if (n === 'render-farm-2023') files(dir(r, 'renders', { meaning: 'media' }), 90, 'frame_seq_', ['exr'], 1.4 * GiB, { modifiedDays: 790 });
    }
    const games = dir(C, 'Games', { meaning: null });
    for (const g of ['Open World', 'Racing Sim', 'Strategy', 'Puzzle Box', 'Flight Sim', 'Arena']) {
      const d = dir(games, g, { meaning: null });
      files(d, 30, 'asset_', ['pak', 'bin', 'bundle'], 1.1 * GiB, { meaning: null, spread: 0.9 });
      files(d, 80, 'cfg_', ['ini', 'dll'], 300 * KiB, { meaning: null });
    }
    const vm = dir(me, 'VMs', { meaning: 'projects' });
    file(vm, 'build-agent.vhdx', 120 * GiB, { ext: 'vhdx', alloc: 74 * GiB, unknowns: ['Sparse disk: allocated is smaller than logical'] });
    file(vm, 'test-win11.vhdx', 64 * GiB, { ext: 'vhdx', alloc: 41 * GiB });
  }

  // ---------------- G: NVMe 930 GB ----------------
  const G = node(root, 'G:', 'volume', { residency: 'g', meaning: null, permission: 'none' });
  Object.assign(G, { label: 'G: · Work NVMe', media: 'NVMe SSD', capacity: 930 * GiB, used: 287 * GiB, coverage: 1, observedMin: 11, scanState: 'complete' });
  {
    const dev = dir(G, 'dev', { meaning: 'projects', permission: 'pinned', collections: ['Active work'] });
    for (const [n, s] of [['loomward', 2.2], ['estate-console', 1.4], ['agent-harness', 1.1], ['taskboard', 1.3], ['field-notes', 0.6]]) repo(dev, n, s);
    const wt = dir(dev, 'worktrees', { permission: 'none', collections: ['Agent worktrees'], unknowns: ['Lease evidence not connected: activity unknown, not "unused"'] });
    for (let k = 0; k < 6; k++) repo(wt, `wt-${['ui0', 'fix-17', 'bench', 'docs', 'sweep', 'probe'][k]}`, 0.35, { permission: 'none' });
    const models = dir(G, 'models', { meaning: 'models' });
    for (const [n, gb, pin] of [['llm-70b-q4', 39.6, true], ['llm-32b-q6', 26.1, true], ['diffusion-xl', 6.9, false], ['speech-large', 3.1, false], ['llm-8b-f16', 15.2, false]]) {
      const d = dir(models, n, { permission: pin ? 'pinned' : 'none', collections: pin ? ['Coding session'] : [] });
      file(d, `${n}.gguf`, gb * GiB, { ext: 'gguf' });
      files(d, 4, 'meta_', ['json', 'txt'], 3 * KiB);
    }
    const ds = dir(G, 'datasets', { meaning: 'projects' });
    files(dir(ds, 'synthetic-inventories'), 400, 'shard_', ['parquet', 'jsonl'], 180 * MiB, { spread: 1.3 });
    files(dir(ds, 'image-sets'), 2600, 'sample_', ['png', 'webp'], 1.1 * MiB);
    const scratch = dir(G, 'scratch', { meaning: 'build', collections: ['Regenerable'] });
    for (let k = 0; k < 8; k++) files(dir(scratch, `build-${k}`), 300, 'obj_', ['o', 'obj', 'pdb', 'tmp'], 2.2 * MiB, { spread: 1.4 });
  }

  // ---------------- E: HDD 1.86 TB, partially scanned ----------------
  const E = node(root, 'E:', 'volume', { residency: 'e', meaning: null, permission: 'none' });
  Object.assign(E, { label: 'E: · Archive HDD', media: '7200 rpm HDD', capacity: 1.86 * TiB, used: 1.51 * TiB, coverage: 0.72, observedMin: 47, scanState: 'paused' });
  {
    const arch = dir(E, 'Archive', { meaning: 'archives' });
    for (const y of [2019, 2020, 2021, 2022, 2023]) {
      const d = dir(arch, `Projects-${y}`, { modifiedDays: (2026 - y) * 365 });
      for (let k = 0; k < 4; k++) files(dir(d, `project-${y}-${k + 1}`), 150, 'file_', ['psd', 'blend', 'zip', 'pdf', 'mp4'], 12 * MiB, { spread: 1.7 });
    }
    const comp = dir(arch, 'Compressed (NTFS)', { unknowns: ['NTFS compression: allocated is ~60% of logical'] });
    for (let i = 0; i < 300; i++) { const s = lsize(40 * MiB, 1.3); file(comp, `bundle_${i}.tar`, s, { ext: 'tar', alloc: Math.round(s * 0.6) }); }
    const media = dir(E, 'Media', { meaning: 'media' });
    const foot = dir(media, 'Footage masters', { permission: 'protected', collections: ['Studio · originals'] });
    files(foot, 60, 'A001_C', ['mov', 'braw'], 4 * GiB, { spread: 1.0 });
    files(dir(media, 'Proxies', { meaning: 'build', collections: ['Regenerable'] }), 160, 'proxy_', ['mp4'], 380 * MiB);
    const photos = dir(media, 'Photo library');
    for (const y of [2018, 2019, 2020, 2021, 2022, 2023, 2024, 2025]) files(dir(photos, String(y)), 900, 'DSC_', ['nef', 'jpg', 'xmp'], 9 * MiB, { spread: 1.1 });
    const bk = dir(E, 'Backups', { meaning: 'archives', permission: 'protected' });
    files(dir(bk, 'Image exports'), 8, 'image_', ['vhdx', 'img'], 30 * GiB, { spread: 0.6, unknowns: ['Restore never tested: snapshot exists is not restore verified'] });
    deniedDir(bk, 'WindowsImageBackup', 'Access denied without elevation: backup contents unmeasured');
  }

  // ---------- aggregate sizes bottom-up; unknown stays unknown ----------
  function total(n) {
    if (!n.children) return;
    let s = 0, a = 0, unm = 0;
    for (const c of n.children) {
      total(c);
      if (c.size == null) unm += 1 + (c.unmeasured || 0);
      else { s += c.size; a += c.alloc ?? 0; unm += c.unmeasured; }
    }
    n.size = n.denied ? null : s;
    n.alloc = n.denied ? null : a;
    n.unmeasured = unm;
    if (unm && !n.denied) n.unknowns.push(`${unm} item${unm > 1 ? 's' : ''} below here have no measured size`);
    // a folder's meaning is the dominant meaning of its bytes when it has none of its own
    if (n.meaning == null && n.kind === 'dir' && n.children.length) {
      const by = {};
      for (const c of n.children) if (c.meaning && c.size) by[c.meaning] = (by[c.meaning] || 0) + c.size;
      const top = Object.entries(by).sort((x, y) => y[1] - x[1])[0];
      if (top && top[1] > 0.6 * s) { n.meaning = top[0]; n.inferredMeaning = true; }
    }
  }
  total(root);

  // volumes: the gap between the volume's used counter and what the scan attributed is real
  // bytes with unknown contents, so it gets area but no meaning, residency claim or permission.
  for (const v of [C, G, E]) {
    const gap = v.used - v.size;
    const g = node(v, v.coverage < 1 ? 'Not yet scanned' : 'Unattributed', 'gap', { meaning: null, residency: null, permission: null });
    g.size = Math.max(0, gap); g.alloc = g.size;
    g.unknowns = [v.coverage < 1
      ? `Scan paused at ${Math.round(v.coverage * 100)}%: size known from the volume counter, contents unknown`
      : 'Volume counter exceeds attributed files: MFT, shadow storage and denied folders'];
    v.size += g.size; v.alloc += g.alloc;
  }
  root.size = C.size + G.size + E.size;
  root.alloc = C.alloc + G.alloc + E.alloc;

  const byId = new Map(all.map((n) => [n.id, n]));
  const find = (path) => path.split('/').reduce((n, part) => n?.children?.find((c) => c.name === part), root);

  // Personal-model review queue: proposals, relative scores, explicit abstentions.
  const review = [
    { path: 'C:/Users/synthetic-user/Downloads/download_0007.7z', label: 'archives', score: 0.81, alts: ['downloads'], why: 'Name pattern and age match 41 items you filed as Archives.' },
    { path: 'C:/Users/synthetic-user/Downloads/document_0012.pdf', label: 'documents', score: 0.74, alts: ['downloads'], why: 'Similar to Invoices: same issuer pattern, 3 confirmed examples.' },
    { path: 'C:/Users/synthetic-user/source/render-farm-2023', label: null, score: null, alts: ['projects', 'archives'], why: 'Abstained: evidence split. Untouched 26 months, but the repo has uncommitted work.' },
    { path: 'G:/models/llm-8b-f16', label: 'models', score: 0.68, alts: ['archives'], why: 'Weights format and folder name. Use frequency is unknown.' },
    { path: 'C:/Users/synthetic-user/Downloads/download_0031.zip', label: null, score: null, alts: ['projects', 'media'], why: 'Abstained: too few labelled neighbours (2) to propose anything.' },
    { path: 'E:/Media/Proxies', label: 'build', score: 0.63, alts: ['media'], why: 'Proxy naming next to masters. Regeneration cost not measured.' },
  ].map((r) => ({ ...r, node: find(r.path) })).filter((r) => r.node);

  // Simulated tier plan: never executable. Before/after in bytes used per volume.
  const cold1 = find('C:/Users/synthetic-user/Videos/2022 exports');
  const cold2 = find('C:/Users/synthetic-user/source/render-farm-2023');
  const tierPlan = {
    simulated: true,
    goal: 'Keep 25% free on C: for builds without touching pinned or protected groups',
    reservePct: 10,
    moves: [
      { node: cold1, from: 'C:', to: 'G:', bytes: cold1.size, reason: 'No modification in 3 years; not pinned. E: lacks room above its reserve', caveat: 'No recent observations is not the same as unused' },
      { node: cold2, from: 'C:', to: 'E:', bytes: cold2.size, reason: 'Cold project group, moved whole', caveat: 'Uncommitted work in the repo: needs your review first' },
    ],
    kept: ['G:/dev (pinned: Active work)', 'G:/models/llm-70b-q4 (pinned: Coding session)', 'E:/Media/Footage masters (protected)'],
  };

  return { root, volumes: [C, G, E], byId, find, review, tierPlan, nodeCount: all.length, seed, synthetic: true, generatedAt: 'synthetic, seed ' + seed.toString(16) };
}

/** Moving synthetic telemetry. Deterministic per seed; one sample per call. */
export function createTelemetry(seed = 0x7e1e) {
  const rnd = mulberry32(seed);
  const total = 64 * GiB, commitLimit = 96 * GiB;
  let t = 0, cpu = 18, gpu = 9, inUse = 27 * GiB, standby = 21 * GiB, modified = 1.2 * GiB, commit = 38 * GiB, vram = 6.2;
  const io = { c: { r: 40, w: 12 }, g: { r: 18, w: 6 }, e: { r: 2, w: 0.4 } };
  const walk = (v, target, k, noise, lo, hi) => Math.min(hi, Math.max(lo, v + (target - v) * k + (rnd() - 0.5) * noise));
  return {
    total, commitLimit,
    next() {
      t++;
      const build = Math.sin(t / 37) > 0.55;           // periodic synthetic "build" episode
      const render = Math.sin(t / 53 + 1) > 0.7;        // and a GPU episode
      cpu = walk(cpu, build ? 74 : 16, 0.18, 9, 1, 100);
      gpu = walk(gpu, render ? 82 : 7, 0.15, 7, 0, 100);
      vram = walk(vram, render ? 19.5 : 6.4, 0.12, 0.4, 2, 24);
      inUse = walk(inUse, (build ? 41 : 28) * GiB, 0.08, 0.6 * GiB, 18 * GiB, 54 * GiB);
      modified = walk(modified, (build ? 2.6 : 1.1) * GiB, 0.1, 0.2 * GiB, 0.3 * GiB, 4 * GiB);
      standby = Math.max(2 * GiB, Math.min(total - inUse - modified - 0.8 * GiB, walk(standby, 22 * GiB, 0.05, 0.5 * GiB, 2 * GiB, 34 * GiB)));
      commit = walk(commit, inUse + 11 * GiB, 0.1, 0.4 * GiB, 20 * GiB, commitLimit);
      const free = total - inUse - modified - standby;
      io.c.r = walk(io.c.r, build ? 620 : 35, 0.3, 40, 0, 3500); io.c.w = walk(io.c.w, build ? 410 : 10, 0.3, 25, 0, 3000);
      io.g.r = walk(io.g.r, render ? 1400 : 20, 0.25, 50, 0, 3500); io.g.w = walk(io.g.w, 8, 0.3, 6, 0, 3000);
      io.e.r = walk(io.e.r, t % 90 > 70 ? 160 : 1.5, 0.3, 3, 0, 220); io.e.w = walk(io.e.w, 0.4, 0.3, 0.6, 0, 220);
      return {
        t, synthetic: true, cpu, gpu, vram, vramTotal: 24,
        ram: { total, inUse, modified, standby, free, available: standby + free, commit, commitLimit },
        io: { c: { ...io.c }, g: { ...io.g }, e: { ...io.e } },
        episode: build ? 'synthetic build' : render ? 'synthetic render' : 'idle',
      };
    },
  };
}

export function formatBytes(n, digits) {
  if (n == null) return 'unknown';
  if (n === 0) return '0 B';
  const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let i = Math.min(u.length - 1, Math.floor(Math.log(Math.abs(n)) / Math.log(1024)));
  const v = n / 1024 ** i;
  const d = digits ?? (v >= 100 ? 0 : v >= 10 ? 1 : 2);
  return `${v.toFixed(i === 0 ? 0 : d)} ${u[i]}`;
}
export const formatCount = (n) => n == null ? 'unknown' : n.toLocaleString('en-GB');
