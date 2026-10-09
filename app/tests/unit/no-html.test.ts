import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import { describe, expect, it } from 'vitest';

const appRoot = join(import.meta.dirname, '..', '..');
const SKIP = new Set(['node_modules', 'dist', 'tests', 'prototype']);
const walk = (dir: string): string[] =>
  !existsSync(dir)
    ? []
    : readdirSync(dir).flatMap((n) => {
        const p = join(dir, n);
        return SKIP.has(n) ? [] : statSync(p).isDirectory() ? walk(p) : /\.(svelte|ts|js|html)$/.test(n) ? [p] : [];
      });

// docs/41 section 12: hostile file names render as text only. Banned: every route that turns a string into markup or code.
const FORBIDDEN = [/\{@html\b/, /\binnerHTML\b/, /\bouterHTML\b/, /\binsertAdjacentHTML\b/, /\bdocument\.write\b/, /\beval\s*\(/, /new\s+Function\s*\(/, /\bsrcdoc\b/];

describe('no markup injection paths', () => {
  const files = [...walk(join(appRoot, 'src')), ...walk(join(appRoot, 'viz')), join(appRoot, 'index.html')];
  it('scans something', () => expect(files.length).toBeGreaterThan(10));
  for (const re of FORBIDDEN) {
    it(`no ${re.source}`, () => {
      const hits = files.filter((f) => re.test(readFileSync(f, 'utf8'))).map((f) => relative(appRoot, f).split(sep).join('/'));
      expect(hits).toEqual([]);
    });
  }
});
