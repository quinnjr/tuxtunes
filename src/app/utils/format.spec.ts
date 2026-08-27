import { describe, expect, it } from 'vitest';
import { formatByteSize, formatTotalDuration } from './format';

describe('formatTotalDuration', () => {
  it('returns 0:00 for zero or negative', () => {
    expect(formatTotalDuration(0)).toBe('0:00');
    expect(formatTotalDuration(-1)).toBe('0:00');
  });

  it('formats sub-day durations as h:mm:ss', () => {
    expect(formatTotalDuration(1000)).toBe('0:00:01');
    expect(formatTotalDuration(3_661_000)).toBe('1:01:01');
    expect(formatTotalDuration(7_323_000)).toBe('2:02:03');
  });

  it('uses singular "day" at exactly 1 day', () => {
    expect(formatTotalDuration(86_400_000)).toBe('1 day, 0:00:00');
  });

  it('pluralizes "days" past 1', () => {
    expect(formatTotalDuration(2 * 86_400_000)).toBe('2 days, 0:00:00');
  });

  it('formats a multi-day mixed duration', () => {
    const ms = 3 * 86_400_000 + 7 * 3_600_000 + 23 * 60_000 + 45_000;
    expect(formatTotalDuration(ms)).toBe('3 days, 7:23:45');
  });
});

describe('formatByteSize', () => {
  it('returns 0 B for zero or negative', () => {
    expect(formatByteSize(0)).toBe('0 B');
    expect(formatByteSize(-1)).toBe('0 B');
  });

  it('formats bytes with no decimal', () => {
    expect(formatByteSize(512)).toBe('512 B');
  });

  it('formats KiB / MiB / GiB / TiB at the appropriate threshold', () => {
    expect(formatByteSize(1024)).toBe('1.00 KiB');
    expect(formatByteSize(1024 * 1024)).toBe('1.00 MiB');
    expect(formatByteSize(1024 * 1024 * 1024)).toBe('1.00 GiB');
    expect(formatByteSize(1024 ** 4)).toBe('1.00 TiB');
  });

  it('uses 1 decimal place for 10–99 of a unit', () => {
    expect(formatByteSize(15 * 1024)).toBe('15.0 KiB');
  });

  it('uses no decimal places for 100+ of a unit', () => {
    expect(formatByteSize(150 * 1024)).toBe('150 KiB');
  });

  it('caps at the largest unit (TiB)', () => {
    // PiB-scale input still renders in TiB.
    expect(formatByteSize(1024 ** 5)).toBe('1024 TiB');
  });
});
