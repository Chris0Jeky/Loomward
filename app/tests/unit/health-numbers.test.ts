import { readFileSync } from 'node:fs';
import { parse } from 'svelte/compiler';
import { describe, expect, it } from 'vitest';

// Exercise the expressions parsed from the real templates, rather than copies of the guards.
// This follows the view-source harness used by the neighbouring view regression tests.
function numberExpression(view: string, field: string): (value: unknown) => string {
  const source = readFileSync(new URL(`../../src/views/${view}/View.svelte`, import.meta.url), 'utf8');
  const expressions: string[] = [];
  function visit(value: unknown): void {
    if (!value || typeof value !== 'object') return;
    const node = value as Record<string, unknown>;
    if (node.type === 'ExpressionTag') {
      const expression = node.expression as { start: number; end: number };
      const text = source.slice(expression.start, expression.end);
      if (text.includes(field) && text.includes('.toFixed(')) expressions.push(text);
    }
    for (const child of Object.values(node)) visit(child);
  }
  visit(parse(source, { modern: true }));
  expect(expressions).toHaveLength(1);
  const evaluate = new Function('health', 'item', `return (${expressions[0]});`);
  return (value) => evaluate({ engine: { cpu_seconds: value } }, { suggestion: { ranked: [{ score: value }] } });
}
const formatCpuSeconds = numberExpression('health', 'cpu_seconds');
const formatScore = numberExpression('review', '.score');

describe('health/review number guards', () => {
  it('renders unknown for missing or non-numeric cpu_seconds', () => {
    expect(formatCpuSeconds(undefined)).toBe('unknown');
    expect(formatCpuSeconds(null)).toBe('unknown');
    expect(formatCpuSeconds('12.3')).toBe('unknown');
    expect(formatCpuSeconds(NaN)).toBe('unknown');
    expect(formatCpuSeconds(Infinity)).toBe('unknown');
    expect(formatCpuSeconds(-Infinity)).toBe('unknown');
  });

  it('formats finite cpu_seconds with one decimal', () => {
    expect(formatCpuSeconds(12.34)).toBe('12.3 s');
    expect(formatCpuSeconds(0)).toBe('0.0 s');
  });

  it('renders unknown for missing, non-numeric or out-of-range scores', () => {
    expect(formatScore(null)).toBe('unknown');
    expect(formatScore(undefined)).toBe('unknown');
    expect(formatScore('0.5')).toBe('unknown');
    expect(formatScore(NaN)).toBe('unknown');
    expect(formatScore(Infinity)).toBe('unknown');
    expect(formatScore(1.5)).toBe('unknown');
    expect(formatScore(-0.1)).toBe('unknown');
  });

  it('formats finite in-range scores with three decimals', () => {
    expect(formatScore(0.12345)).toBe('0.123');
    expect(formatScore(0)).toBe('0.000');
    expect(formatScore(1)).toBe('1.000');
  });
});
