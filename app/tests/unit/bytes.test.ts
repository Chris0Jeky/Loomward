import { describe, expect, it } from 'vitest';
import { bytesToNumber, formatBytes, parseBytes } from '../../src/lib/format/bytes';
import { formatCount, formatTime } from '../../src/lib/format/time';

describe('bytes', () => {
  it('parses only exact unsigned decimal strings', () => {
    expect(parseBytes('0')).toBe(0n);
    expect(parseBytes('18446744073709551615')).toBe(18446744073709551615n);
    for (const bad of ['01', '-1', '1.5', '1e3', ' 1', '', '123456789012345678901', null, undefined]) {
      expect(parseBytes(bad as string | null)).toBeNull();
    }
  });
  it('keeps unknown unknown, never zero', () => {
    expect(formatBytes(null)).toBe('unknown');
    expect(formatBytes('not-a-number')).toBe('unknown');
    expect(formatBytes('0')).toBe('0 B');
    expect(bytesToNumber(null)).toBeNull();
  });
  it('formats binary units', () => {
    expect(formatBytes('1023')).toBe('1023 B');
    expect(formatBytes('1024')).toBe('1.00 KiB');
    expect(formatBytes(String(5n * 1024n ** 3n))).toBe('5.00 GiB');
    expect(formatBytes('18446744073709551615')).toBe('16.0 EiB');
  });
  it('formats time and counts with unknown for null', () => {
    expect(formatTime('2026-10-01T09:00:00Z')).toBe('2026-10-01 09:00 UTC');
    expect(formatTime(null)).toBe('unknown');
    expect(formatTime('garbage')).toBe('unknown');
    expect(formatCount(null)).toBe('unknown');
    expect(formatCount(0)).toBe('0');
  });
});
