import appFixture from '../../../../contracts/v3/examples/commands/apps.list.result.json';
import desktopFixture from '../../../../contracts/v3/examples/commands/desktop.contexts.result.json';
import type { ApplicationPage, DesktopPage, ErrorCode, InstalledApplication } from '../contracts.gen';

/** Largest provider estimate first; an unknown size sorts last, never as zero. Stable, so ties keep registry order. */
export function sortLargestFirst(rows: InstalledApplication[]): InstalledApplication[] {
  const size = (a: InstalledApplication) => (a.estimated_size_bytes === null ? null : BigInt(a.estimated_size_bytes));
  return rows
    .map((row, i) => ({ row, i, n: size(row) }))
    .sort((a, b) => (a.n === null ? (b.n === null ? 0 : 1) : b.n === null ? -1 : a.n === b.n ? 0 : a.n > b.n ? -1 : 1) || a.i - b.i)
    .map((e) => e.row);
}

/** `source` replaces the contract fixtures; tests use it to seed a snapshot whose registry order differs from size order. */
export function createInventoryMock(now: () => Date, fail: (code: ErrorCode, message: string) => never, source: { apps?: unknown; desktop?: unknown } = {}) {
  type Page = ApplicationPage | DesktopPage;
  const cursors = new Map<string, { command: string; page: Page; offset: number; sort: string }>();
  const prefix = `cu${crypto.randomUUID().replaceAll('-', '')}`;
  let serial = 0;

  function page(command: 'apps.list' | 'desktop.contexts', p: Record<string, unknown>): Page {
    // Absent means registry; an explicit null is not a sort, so the service's closed set rejects it below.
    const sort = p.sort === undefined ? 'registry' : p.sort;
    const allowed = command === 'apps.list' ? ['limit', 'cursor', 'sort'] : ['limit', 'cursor'];
    if (Object.keys(p).some((k) => !allowed.includes(k)) || (sort !== 'registry' && sort !== 'size_desc') || !Number.isInteger(p.limit) || Number(p.limit) < 1 || Number(p.limit) > 200 || !('cursor' in p) || (p.cursor !== null && typeof p.cursor !== 'string')) {
      return fail('invalid_request', 'Expected limit 1–200 and nullable cursor');
    }
    let snapshot: Page, offset = 0;
    if (p.cursor !== null) {
      const saved = cursors.get(p.cursor as string) ?? fail('stale_generation', 'Metadata cursor expired; refresh the view');
      if (saved.command !== command) return fail('invalid_request', 'Cursor belongs to another command');
      if (saved.sort !== sort) return fail('invalid_request', 'Cursor was issued for another sort');
      snapshot = saved.page;
      offset = saved.offset;
    } else {
      snapshot = structuredClone(command === 'apps.list' ? (source.apps ?? appFixture) : (source.desktop ?? desktopFixture)) as Page;
      snapshot.observed_at = now().toISOString();
      if ('applications' in snapshot && sort === 'size_desc') snapshot.applications = sortLargestFirst(snapshot.applications);
    }
    const key = command === 'apps.list' ? 'applications' : 'shortcuts';
    const rows = 'applications' in snapshot ? snapshot.applications : snapshot.shortcuts;
    const end = Math.min(offset + Number(p.limit), rows.length);
    let next: string | null = null;
    if (end < rows.length) {
      next = `${prefix}_${++serial}`;
      if (cursors.size >= 16) cursors.delete(cursors.keys().next().value!);
      cursors.set(next, { command, page: snapshot, offset: end, sort });
    }
    return { ...snapshot, [key]: rows.slice(offset, end), next_cursor: next };
  }
  return {
    'apps.list': (p: Record<string, unknown>) => page('apps.list', p),
    'desktop.contexts': (p: Record<string, unknown>) => page('desktop.contexts', p),
  };
}
