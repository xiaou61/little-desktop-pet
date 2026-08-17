import { describe, expect, it } from 'vitest';

import {
  addLocalDays,
  canNavigateForward,
  formatCalendarDate,
  nextLocalDate,
  parseLocalDate,
  toLocalDateString
} from '../lib/date';

describe('local date helpers', () => {
  it('uses local calendar fields without UTC conversion', () => {
    const local = new Date(2026, 7, 14, 0, 30);
    expect(toLocalDateString(local)).toBe('2026-08-14');
    expect(parseLocalDate('2026-08-14').getDate()).toBe(14);
  });

  it('navigates across month boundaries and never advances past today', () => {
    expect(addLocalDays('2026-08-01', -1)).toBe('2026-07-31');
    expect(nextLocalDate('2026-08-13', '2026-08-14')).toBe('2026-08-14');
    expect(nextLocalDate('2026-08-14', '2026-08-14')).toBe('2026-08-14');
    expect(canNavigateForward('2026-08-13', '2026-08-14')).toBe(true);
    expect(canNavigateForward('2026-08-14', '2026-08-14')).toBe(false);
  });

  it('rejects invalid dates and labels today', () => {
    expect(() => parseLocalDate('2026-02-30')).toThrow();
    expect(formatCalendarDate('2026-08-14', '2026-08-14')).toContain('今天');
  });
});

