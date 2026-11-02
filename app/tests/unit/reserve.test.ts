import { expect, it } from 'vitest';
import { reserveBytes, reserveDraft } from '../../src/views/tiers/reserve';

it('converts whole GiB and preserves byte values beyond Number precision', () => {
  expect(reserveBytes('80', 'GiB')).toBe('85899345920');
  expect(reserveBytes('60', 'GiB')).toBe('64424509440');
  expect(reserveBytes('18446744073709551615', 'B')).toBe('18446744073709551615');
  expect(reserveDraft('9007199254740993')).toEqual({ value: '9007199254740993', unit: 'B' });
  expect(reserveDraft('85899345920')).toEqual({ value: '80', unit: 'GiB' });
  expect(reserveDraft(null)).toEqual({ value: '', unit: 'GiB' });
  expect(reserveDraft('0')).toEqual({ value: '0', unit: 'GiB' });
});

it('refuses malformed, fractional and overflowing reserves', () => {
  for (const value of ['', '-1', '+1', '01', '1.5', '1e3', ' 1 ', '18446744073709551616']) {
    expect(reserveBytes(value, 'B')).toBeNull();
  }
  expect(reserveBytes('17179869184', 'GiB')).toBeNull();
});
