import { describe, expect, it } from 'vitest';
import { formatMmSs } from './time';

describe('formatMmSs', () => {
  it('formats whole seconds with zero-padded seconds', () => {
    expect(formatMmSs(0)).toBe('0:00');
    expect(formatMmSs(5000)).toBe('0:05');
    expect(formatMmSs(65_000)).toBe('1:05');
  });

  it('rounds millisecond fractions to the nearest second', () => {
    expect(formatMmSs(499)).toBe('0:00');
    expect(formatMmSs(500)).toBe('0:01');
    expect(formatMmSs(1499)).toBe('0:01');
    expect(formatMmSs(1500)).toBe('0:02');
  });

  it('handles minute rollover', () => {
    expect(formatMmSs(59_999)).toBe('1:00');
    expect(formatMmSs(60_000)).toBe('1:00');
    expect(formatMmSs(3_600_000)).toBe('60:00');
  });

  it('clamps negative inputs to zero', () => {
    expect(formatMmSs(-100)).toBe('0:00');
    expect(formatMmSs(-1)).toBe('0:00');
  });
});
