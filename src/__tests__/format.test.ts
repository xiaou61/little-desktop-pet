import { describe, expect, it } from 'vitest';

import { formatDuration, formatPercentage } from '../lib/format';

describe('display formatting', () => {
  it('formats seconds, minutes, and hours compactly', () => {
    expect(formatDuration(0)).toBe('0 分钟');
    expect(formatDuration(12_000)).toBe('12 秒');
    expect(formatDuration(5 * 60_000)).toBe('5 分钟');
    expect(formatDuration(3_900_000)).toBe('1 小时 5 分钟');
  });

  it('clamps percentage values to the UI contract', () => {
    expect(formatPercentage(0.125)).toBe('12.5%');
    expect(formatPercentage(2)).toBe('100%');
    expect(formatPercentage(Number.NaN)).toBe('0%');
  });
});

