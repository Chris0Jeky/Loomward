// Every "no granted folder" empty state links to where a folder is granted (#1015).
import { expect, it } from 'vitest';

const views = import.meta.glob('../../src/views/**/*.svelte', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

it('each no-grant empty state links to Grants & health', () => {
  const empties = Object.entries(views)
    .filter(([path]) => !path.includes('/health/'))
    .flatMap(([path, text]) => (text.match(/<p class="empty">[^\n]*?<\/p>/g) ?? []).map((p) => [path, p] as const))
    .filter(([, p]) => /no granted/i.test(p));
  expect(empties.length).toBeGreaterThanOrEqual(4);
  for (const [path, p] of empties) expect(p, path).toContain('href="#/health"');
});
