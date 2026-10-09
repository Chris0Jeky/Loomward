// `tsc --checkJs --noEmit` over app/viz (ADR-V3-11). Lane L10 owns app/viz; until it lands there
// is nothing to check, and tsc would fail with TS18003 on an empty include.
import { existsSync, readdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const viz = join(root, 'viz');
const hasJs = (d) =>
  readdirSync(d, { withFileTypes: true }).some((e) =>
    e.isDirectory() ? hasJs(join(d, e.name)) : /\.(js|d\.ts)$/.test(e.name),
  );
if (!existsSync(viz) || !hasJs(viz)) {
  console.log('check-viz: app/viz has no modules yet; skipped (lane L10)');
  process.exit(0);
}
const tsc = join(root, 'node_modules', 'typescript', 'bin', 'tsc');
const r = spawnSync(process.execPath, [tsc, '-p', join(root, 'tsconfig.viz.json')], { stdio: 'inherit' });
process.exit(r.status ?? 1);
