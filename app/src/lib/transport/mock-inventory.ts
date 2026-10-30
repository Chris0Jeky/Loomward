import appFixture from '../../../../contracts/v3/examples/commands/apps.list.result.json';
import desktopFixture from '../../../../contracts/v3/examples/commands/desktop.contexts.result.json';
import type { ApplicationPage, DesktopPage, ErrorCode, InstalledApplication } from '../contracts.gen';

/** `size_desc`: largest estimate first, unknown last (never zero), ties keep their order (sort is stable). */
export function sizeDesc(rows: readonly InstalledApplication[]): InstalledApplication[] {
  const size = (a: InstalledApplication) => (a.estimated_size_bytes === null ? null : BigInt(a.estimated_size_bytes));
  return [...rows].sort((a, b) => {
    const x = size(a), y = size(b);
    if (x === null || y === null) return x === y ? 0 : x === null ? 1 : -1;
    return x > y ? -1 : x < y ? 1 : 0;
  });
}

export function createInventoryMock(now: () => Date, fail: (code: ErrorCode, message: string) => never) {
  type Page = ApplicationPage | DesktopPage;
  const cursors = new Map<string, { command: string; page: Page; offset: number; sort: string | null }>();
  const prefix = `cu${crypto.randomUUID().replaceAll('-', '')}`;
  let serial = 0;

  function page(command: 'apps.list' | 'desktop.contexts', p: Record<string, unknown>): Page {
    const sort = p.sort ?? null;
    const allowed = command === 'apps.list' ? ['limit', 'cursor', 'sort'] : ['limit', 'cursor'];
    if (sort !== null && (command !== 'apps.list' || sort !== 'size_desc')) return fail('invalid_request', 'Expected sort size_desc');
    if (Object.keys(p).some((k) => !allowed.includes(k)) || !Number.isInteger(p.limit) || Number(p.limit) < 1 || Number(p.limit) > 200 || !('cursor' in p) || (p.cursor !== null && typeof p.cursor !== 'string')) {
      return fail('invalid_request', 'Expected limit 1–200 and nullable cursor');
    }
    let snapshot: Page, offset = 0;
    if (p.cursor !== null) {
      const saved = cursors.get(p.cursor as string) ?? fail('stale_generation', 'Metadata cursor expired; refresh the view');
      if (saved.command !== command) return fail('invalid_request', 'Cursor belongs to another command');
      if (saved.sort !== sort) return fail('invalid_request', 'Cursor belongs to another sort order');
      snapshot = saved.page;
      offset = saved.offset;
    } else {
      snapshot = structuredClone(command === 'apps.list' ? appFixture : desktopFixture) as Page;
      snapshot.observed_at = now().toISOString();
      if (sort !== null) (snapshot as ApplicationPage).applications = sizeDesc((snapshot as ApplicationPage).applications);
    }
    const key = command === 'apps.list' ? 'applications' : 'shortcuts';
    const rows = 'applications' in snapshot ? snapshot.applications : snapshot.shortcuts;
    const end = Math.min(offset + Number(p.limit), rows.length);
    let next: string | null = null;
    if (end < rows.length) {
      next = `${prefix}_${++serial}`;
      if (cursors.size >= 16) cursors.delete(cursors.keys().next().value!);
      cursors.set(next, { command, page: snapshot, offset: end, sort: sort as string | null });
    }
    return { ...snapshot, [key]: rows.slice(offset, end), next_cursor: next };
  }
  return {
    'apps.list': (p: Record<string, unknown>) => page('apps.list', p),
    'desktop.contexts': (p: Record<string, unknown>) => page('desktop.contexts', p),
  };
}
