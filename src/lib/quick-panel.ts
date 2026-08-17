import type { DailyApplicationUsage } from './types';

export function topApplications(items: DailyApplicationUsage[]): DailyApplicationUsage[] {
  return [...items]
    .sort(
      (left, right) =>
        right.activeMs - left.activeMs ||
        left.displayName.localeCompare(right.displayName, 'zh-CN') ||
        left.executableName.localeCompare(right.executableName, 'en')
    )
    .slice(0, 3);
}

export function normalizePetSize(value: number): number {
  return Number.isInteger(value) && value >= 30 && value <= 160 && value % 10 === 0
    ? value
    : 100;
}
