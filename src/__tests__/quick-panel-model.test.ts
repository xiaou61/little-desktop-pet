import { describe, expect, it } from 'vitest';

import { normalizePetSize, topApplications } from '../lib/quick-panel';

describe('quick panel model', () => {
  it('sorts deterministically and returns at most three application summaries', () => {
    const applications = topApplications([
      { displayName: '浏览器', executableName: 'browser.exe', activeMs: 2_000, share: 0.2 },
      { displayName: '终端', executableName: 'terminal.exe', activeMs: 5_000, share: 0.5 },
      { displayName: '编辑器', executableName: 'editor.exe', activeMs: 5_000, share: 0.5 },
      { displayName: '音乐', executableName: 'music.exe', activeMs: 1_000, share: 0.1 }
    ]);

    expect(applications.map((application) => application.displayName)).toEqual([
      '编辑器',
      '终端',
      '浏览器'
    ]);
  });

  it('keeps valid size steps and normalizes every unsupported value to 100', () => {
    for (let value = 30; value <= 160; value += 10) {
      expect(normalizePetSize(value)).toBe(value);
    }
    for (const value of [-1, 20, 29, 31, 105, 161, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(normalizePetSize(value)).toBe(100);
    }
  });
});
