import { describe, expect, it } from 'vitest';
import { vramGiB, uniqueLabels } from '../../src/views/observatory/telemetry';

describe('vramGiB', () => {
  const GiB = String(2 ** 30);
  it('reads used against the adapter total', () => {
    expect(vramGiB(String(2 * 2 ** 30), String(8 * 2 ** 30))).toEqual({ value: 2, totalGiB: 8 });
  });
  it('a total of 0 is no scale: unknown, not a value against a stale range', () => {
    expect(vramGiB(GiB, '0')).toEqual({ value: null, totalGiB: null });
  });
  it('a missing total or used is unknown', () => {
    expect(vramGiB(GiB, null).value).toBeNull();
    expect(vramGiB(null, GiB).value).toBeNull();
    expect(vramGiB('garbage', GiB).value).toBeNull();
  });
});

describe('uniqueLabels', () => {
  it('keeps distinct labels and marks repeats visibly', () => {
    expect(uniqueLabels(['C:', 'D:'])).toEqual(['C:', 'D:']);
    expect(uniqueLabels(['C:', 'C:', 'C:'])).toEqual(['C:', 'C: (2)', 'C: (3)']);
  });
  it('a repeat never collides with a real label that looks like the marker', () => {
    const out = uniqueLabels(['C:', 'C:', 'C: (2)']);
    expect(new Set(out).size).toBe(3);
  });
});
