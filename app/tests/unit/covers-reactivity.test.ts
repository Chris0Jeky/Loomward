import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { compileModule } from 'svelte/compiler';
import ts from 'typescript';
import { expect, it, vi } from 'vitest';
import type { Covers } from '../../src/lib/stores/covers.svelte';
import { isActive } from '../../src/lib/scan/model';
import { LoomwardError } from '../../src/lib/transport/client';
import { coversRefusal } from '../../src/views/duplicates/covers-model';

// The normal Node tests exercise getters directly. Compile the actual store for the client so
// this regression also observes the dependency tracking used by the panel's conditional block.
it('notifies the rendered retry condition when a held completed-result read fails', async () => {
  const runtimeUrl = pathToFileURL(createRequire(import.meta.url).resolve('svelte/internal/client')).href;
  const runtime = await import(/* @vite-ignore */ runtimeUrl);
  const source = readFileSync(new URL('../../src/lib/stores/covers.svelte.ts', import.meta.url), 'utf8');
  const javascript = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
  }).outputText.replace(/^import .*;\n/gm, '');
  const compiled = compileModule(javascript, { filename: 'covers.svelte.js', generate: 'client', dev: false }).js.code
    .replace(/^import .*;\n/gm, '').replace('export class Covers', 'class Covers');
  let reject!: (reason: Error) => void;
  const response = new Promise<never>((_, fail) => { reject = fail; });
  const call = vi.fn(() => response);
  const session = { client: { call }, info: null, handle: (e: Error) => e.message };
  const ClientCovers = new Function('$', 'session', 'isActive', 'LoomwardError', 'coversRefusal', `${compiled}\nreturn Covers;`)(
    runtime, session, isActive, LoomwardError, coversRefusal,
  ) as new () => Covers;
  const store = new ClientCovers();
  const observed: boolean[] = [];
  const dispose = runtime.effect_root(() => runtime.render_effect(() => { observed.push(store.canRetryResult); }));
  try {
    store.apply({ job_id: 'jb_retry', kind: 'coverage', state: 'completed', seq: 1, rev: 1, root_id: null,
      created_at: '2026-10-30T09:00:00Z', started_at: '2026-10-30T09:00:00Z', finished_at: '2026-10-30T09:01:00Z',
      progress: null, coverage: null, error: null });
    runtime.flush();
    expect(observed.at(-1)).toBe(false);
    reject(new LoomwardError({ code: 'internal_error', message: 'Synthetic transient result failure', retryable: true, detail: null }));
    await response.catch(() => {});
    runtime.flush();
    // This passes even with a plain, nonreactive fetched field: direct getters missed the bug.
    expect(store.canRetryResult).toBe(true);
    expect(store.problem).toBe('Synthetic transient result failure');
    expect(observed.at(-1)).toBe(true);
    expect(call.mock.calls).toEqual([['coverage.result', { job_id: 'jb_retry' }]]);
  } finally { dispose(); }
});
