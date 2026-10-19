// Explorer non-string entry flags: a row with flags [123, 'UNKNOWN_FLAG'] must render
// its flag badges as text instead of throwing (f.replaceAll on a number is a TypeError
// that crashes the table). The test evaluates the flag-label expression used by
// app/src/views/explorer/View.svelte so it fails on the unfixed template and passes
// once each flag is coerced with String(f) before render.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

function flagLabelExpression(): string {
  const source = readFileSync(new URL('../../src/views/explorer/View.svelte', import.meta.url), 'utf8');
  const match = source.match(/\{#each r\.flags as f \(f\)\}<span class="tag">\{([^}]+)\}<\/span>/);
  if (!match) throw new Error('Missing explorer flags badge expression in View.svelte');
  return match[1]!.trim();
}

function renderFlags(flags: unknown[]): string[] {
  const renderOne = new Function('f', `return (${flagLabelExpression()});`) as (f: unknown) => string;
  return flags.map((f) => renderOne(f));
}

describe('explorer flag badges', () => {
  it('renders non-string flags as text without throwing', () => {
    const flags: unknown[] = [123, 'UNKNOWN_FLAG'];
    let rendered: string[];
    expect(() => {
      rendered = renderFlags(flags);
    }).not.toThrow();
    expect(rendered!).toEqual(['123', 'UNKNOWN FLAG']);
  });

  it('keeps the existing display for string flags', () => {
    expect(renderFlags(['SOME_FLAG'])).toEqual(['SOME FLAG']);
  });
});
