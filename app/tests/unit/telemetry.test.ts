import { describe, expect, it } from 'vitest';
import { byteCount, vramGiB, vramLabel, uniqueLabels } from '../../src/views/observatory/telemetry';

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

describe('vramLabel', () => {
  const GiB = String(2 ** 30);
  it('says unknown when the total is 0, missing or malformed: the dial reads unknown, so the text does not say "0 B"', () => {
    for (const total of ['0', null, undefined, 'garbage']) {
      const label = vramLabel(GiB, total);
      expect(label).toContain('unknown');
      expect(label).not.toContain('0 B');
    }
  });
  it('reads used of total when the adapter reports a total', () => {
    expect(vramLabel(String(2 * 2 ** 30), String(8 * 2 ** 30))).toBe('GPU memory used 2.00 GiB of 8.00 GiB');
  });
});

describe('byteCount', () => {
  it('follows parseBytes: no leading zeros, no more than 20 digits', () => {
    expect(byteCount('1024')).toBe(1024);
    expect(byteCount('0')).toBe(0);
    for (const bad of ['007', '1'.repeat(21), '-1', '1.5', '', null, undefined]) expect(byteCount(bad as string)).toBeNull();
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
