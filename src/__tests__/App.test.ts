import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import App from '../App.svelte';
import { addLocalDays, toLocalDateString } from '../lib/date';
import type { DailyUsageSummary } from '../lib/types';
import { fetchDailyUsage, fetchTrackerStatus } from '../lib/api';

vi.mock('../lib/api', () => ({
  fetchDailyUsage: vi.fn(),
  fetchTrackerStatus: vi.fn(),
  closeDashboard: vi.fn().mockResolvedValue(undefined)
}));

const fetchUsageMock = vi.mocked(fetchDailyUsage);
const fetchStatusMock = vi.mocked(fetchTrackerStatus);

function summary(overrides: Partial<DailyUsageSummary> = {}): DailyUsageSummary {
  return {
    date: toLocalDateString(),
    trackerState: 'recording',
    totalActiveMs: 12_000,
    applications: [
      {
        displayName: '浏览器',
        executableName: 'browser.exe',
        activeMs: 2_000,
        share: 2 / 12
      },
      {
        displayName: '编辑器',
        executableName: 'editor.exe',
        activeMs: 10_000,
        share: 10 / 12
      }
    ],
    ...overrides
  };
}

beforeEach(() => {
  fetchUsageMock.mockReset();
  fetchStatusMock.mockReset();
  fetchStatusMock.mockResolvedValue({ state: 'error', message: '本地存储暂时不可用。' });
});

describe('daily dashboard', () => {
  it('shows pending-inclusive totals and deterministically sorted rows', async () => {
    fetchUsageMock.mockResolvedValue(summary());
    render(App);

    expect(await screen.findByTestId('daily-total')).toHaveTextContent('12 秒');
    const rows = screen.getAllByTestId('app-row');
    expect(within(rows[0]).getByText('编辑器')).toBeInTheDocument();
    expect(within(rows[1]).getByText('浏览器')).toBeInTheDocument();
    expect(screen.getByText('83.3%')).toBeInTheDocument();
  });

  it('renders an empty selected date without fabricated rows', async () => {
    fetchUsageMock.mockResolvedValue(
      summary({ totalActiveMs: 0, applications: [], trackerState: 'idle' })
    );
    render(App);

    expect(await screen.findByText('这一天还没有记录')).toBeInTheDocument();
    expect(screen.queryAllByTestId('app-row')).toHaveLength(0);
    expect(screen.getByTestId('daily-total')).toHaveTextContent('0 分钟');
  });

  it('shows a bounded persistence error and supports retry', async () => {
    fetchUsageMock
      .mockRejectedValueOnce(new Error('database path must not leak'))
      .mockResolvedValueOnce(summary());
    render(App);

    expect(await screen.findByText('本地存储暂时不可用。')).toBeInTheDocument();
    expect(screen.queryByText('database path must not leak')).not.toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: '重试' }));
    expect(await screen.findByText('编辑器')).toBeInTheDocument();
  });

  it('navigates backward, returns to today, and disables future navigation', async () => {
    fetchUsageMock.mockImplementation(async (date) => summary({ date }));
    render(App);
    await screen.findByText('编辑器');

    const today = toLocalDateString();
    const previous = addLocalDays(today, -1);
    await fireEvent.click(screen.getByRole('button', { name: '前一天' }));
    await waitFor(() => expect(fetchUsageMock).toHaveBeenCalledWith(previous));
    expect(screen.getByRole('button', { name: '后一天' })).not.toBeDisabled();

    await fireEvent.click(screen.getByRole('button', { name: '后一天' }));
    await waitFor(() => expect(fetchUsageMock).toHaveBeenCalledWith(today));
    expect(screen.getByRole('button', { name: '后一天' })).toBeDisabled();
  });
});

