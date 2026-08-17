import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import QuickPanel from '../QuickPanel.svelte';
import type { DailyUsageSummary, PetSizeUpdate, PetSkinOption } from '../lib/types';
import {
  applyPetSize,
  applyPetSkin,
  closeQuickPanel,
  fetchDailyUsage,
  fetchCurrentPetSkin,
  fetchPetSize,
  fetchPetSkins,
  fetchQuickPanelEnvironment,
  markQuickPanelInternalAction,
  notifyQuickPanelReady,
  openFullStatistics,
  previewPetSize
} from '../lib/api';

vi.mock('../lib/api', () => ({
  applyPetSize: vi.fn(),
  applyPetSkin: vi.fn(),
  closeQuickPanel: vi.fn().mockResolvedValue(undefined),
  fetchDailyUsage: vi.fn(),
  fetchCurrentPetSkin: vi.fn(),
  fetchPetSize: vi.fn(),
  fetchPetSkins: vi.fn(),
  fetchQuickPanelEnvironment: vi.fn(),
  markQuickPanelInternalAction: vi.fn().mockResolvedValue(undefined),
  notifyQuickPanelReady: vi.fn().mockResolvedValue(undefined),
  openFullStatistics: vi.fn().mockResolvedValue(undefined),
  previewPetSize: vi.fn()
}));

const fetchUsageMock = vi.mocked(fetchDailyUsage);
const fetchPetSizeMock = vi.mocked(fetchPetSize);
const environmentMock = vi.mocked(fetchQuickPanelEnvironment);
const applyPetSizeMock = vi.mocked(applyPetSize);
const previewPetSizeMock = vi.mocked(previewPetSize);
const applyPetSkinMock = vi.mocked(applyPetSkin);
const fetchCurrentPetSkinMock = vi.mocked(fetchCurrentPetSkin);
const fetchPetSkinsMock = vi.mocked(fetchPetSkins);

const petSkins: PetSkinOption[] = [
  {
    id: 'simple-cloud',
    displayName: '简洁云朵',
    thumbnailDataUrl: 'data:image/png;base64,cloud',
    available: true
  },
  {
    id: 'orange-dragon',
    displayName: '橙色小龙',
    thumbnailDataUrl: 'data:image/png;base64,dragon',
    available: true
  },
  {
    id: 'calico-cat',
    displayName: '三花猫',
    thumbnailDataUrl: 'data:image/png;base64,cat',
    available: true
  }
];

function summary(overrides: Partial<DailyUsageSummary> = {}): DailyUsageSummary {
  return {
    date: '2026-08-15',
    trackerState: 'recording',
    totalActiveMs: 13_000,
    applications: [
      { displayName: '浏览器', executableName: 'browser.exe', activeMs: 2_000, share: 2 / 13 },
      { displayName: '终端', executableName: 'terminal.exe', activeMs: 4_000, share: 4 / 13 },
      { displayName: '编辑器', executableName: 'editor.exe', activeMs: 6_000, share: 6 / 13 },
      { displayName: '音乐', executableName: 'music.exe', activeMs: 1_000, share: 1 / 13 }
    ],
    ...overrides
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  fetchUsageMock.mockResolvedValue(summary());
  fetchPetSizeMock.mockResolvedValue({ sizePercent: 100 });
  fetchPetSkinsMock.mockResolvedValue(petSkins);
  fetchCurrentPetSkinMock.mockResolvedValue({ skinId: 'simple-cloud' });
  environmentMock.mockResolvedValue({
    glassAvailable: true,
    highContrast: false,
    reduceMotion: true,
    lastError: null
  });
  applyPetSizeMock.mockImplementation(async (sizePercent) => ({
    sizePercent,
    saved: true,
    message: null
  }));
  previewPetSizeMock.mockImplementation(async (sizePercent) => ({ sizePercent }));
  applyPetSkinMock.mockImplementation(async (skinId) => ({
    skinId,
    saved: true,
    message: null
  }));
});

describe('pet quick panel', () => {
  it('shows today status, total, and only the three longest application summaries', async () => {
    render(QuickPanel);

    expect(await screen.findByTestId('quick-total')).toHaveTextContent('13 秒');
    expect(screen.getByText('正在记录')).toBeInTheDocument();
    const rows = screen.getAllByTestId('quick-app-row');
    expect(rows).toHaveLength(3);
    expect(within(rows[0]).getByText('编辑器')).toBeInTheDocument();
    expect(screen.queryByText('音乐')).not.toBeInTheDocument();
  });

  it('renders a real empty state and a bounded read error without fabricated rows', async () => {
    fetchUsageMock.mockResolvedValueOnce(summary({ totalActiveMs: 0, applications: [] }));
    const { unmount } = render(QuickPanel);
    expect(await screen.findByText('今天还没有使用记录')).toBeInTheDocument();
    expect(screen.queryAllByTestId('quick-app-row')).toHaveLength(0);
    unmount();

    fetchUsageMock.mockRejectedValueOnce(new Error('C:\\private\\usage.sqlite3'));
    render(QuickPanel);
    expect(await screen.findByRole('alert')).toHaveTextContent('今日概览暂时无法读取');
    expect(screen.queryByText(/private/)).not.toBeInTheDocument();
  });

  it('provides semantic close, statistics, settings, and size controls', async () => {
    render(QuickPanel);
    await screen.findByTestId('quick-total');
    expect(screen.getByRole('button', { name: '关闭快捷面板' })).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: '查看完整统计' }));
    expect(openFullStatistics).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByRole('button', { name: '设置' }));
    const slider = await screen.findByRole('slider', { name: '桌宠大小' });
    expect(slider).toHaveAttribute('min', '30');
    expect(slider).toHaveAttribute('max', '160');
    expect(slider).toHaveAttribute('step', '10');

    await fireEvent.input(slider, { target: { value: '30' } });
    await waitFor(() => expect(previewPetSizeMock).toHaveBeenCalledWith(30));
    expect(applyPetSizeMock).not.toHaveBeenCalled();
    await fireEvent.change(slider, { target: { value: '30' } });
    await waitFor(() => expect(applyPetSizeMock).toHaveBeenCalledWith(30));
    await waitFor(() => expect(slider).toHaveValue('30'));
  });

  it('shows the offline local skin gallery with cloud selected by default', async () => {
    render(QuickPanel);
    await fireEvent.click(await screen.findByRole('button', { name: '设置' }));

    expect(await screen.findByRole('button', { name: '选择简洁云朵' })).toHaveAttribute(
      'aria-pressed',
      'true'
    );
    expect(screen.getByRole('button', { name: '选择橙色小龙' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '选择三花猫' })).toBeInTheDocument();
    expect(screen.getByText('当前')).toBeInTheDocument();
    expect(fetchPetSkinsMock).toHaveBeenCalledTimes(1);
  });

  it('applies a selected skin immediately and does not repeat the current choice', async () => {
    render(QuickPanel);
    await fireEvent.click(await screen.findByRole('button', { name: '设置' }));
    const cloud = screen.getByRole('button', { name: '选择简洁云朵' });
    await fireEvent.click(cloud);
    expect(applyPetSkinMock).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByRole('button', { name: '选择橙色小龙' }));
    await waitFor(() => expect(applyPetSkinMock).toHaveBeenCalledWith('orange-dragon'));
    expect(await screen.findByText('已应用橙色小龙')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '选择橙色小龙' })).toHaveAttribute(
      'aria-pressed',
      'true'
    );
  });

  it('keeps the previous selection and announces a failed skin switch', async () => {
    applyPetSkinMock.mockRejectedValueOnce({ code: 'pet_skin_frame_failed' });
    render(QuickPanel);
    await fireEvent.click(await screen.findByRole('button', { name: '设置' }));
    await fireEvent.click(screen.getByRole('button', { name: '选择三花猫' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('皮肤切换失败');
    expect(screen.getByRole('button', { name: '选择简洁云朵' })).toHaveAttribute(
      'aria-pressed',
      'true'
    );
    expect(screen.getByRole('button', { name: '选择三花猫' })).toHaveAttribute(
      'aria-pressed',
      'false'
    );
  });

  it('keeps live size input responsive and coalesces queued rebuilds to the latest value', async () => {
    let resolveFirst!: (result: PetSizeUpdate) => void;
    previewPetSizeMock.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveFirst = resolve;
        })
    );
    render(QuickPanel);
    await fireEvent.click(await screen.findByRole('button', { name: '设置' }));
    const slider = screen.getByRole('slider', { name: '桌宠大小' });

    await fireEvent.input(slider, { target: { value: '110' } });
    expect(previewPetSizeMock).toHaveBeenCalledTimes(1);
    expect(previewPetSizeMock).toHaveBeenLastCalledWith(110);

    await fireEvent.input(slider, { target: { value: '120' } });
    await fireEvent.input(slider, { target: { value: '140' } });
    expect(slider).not.toBeDisabled();
    expect(previewPetSizeMock).toHaveBeenCalledTimes(1);

    resolveFirst({ sizePercent: 110, saved: true, message: null });
    await waitFor(() => expect(previewPetSizeMock).toHaveBeenCalledTimes(2));
    expect(previewPetSizeMock).toHaveBeenLastCalledWith(140);
    expect(previewPetSizeMock).not.toHaveBeenCalledWith(120);
    await fireEvent.change(slider, { target: { value: '140' } });
    await waitFor(() => expect(applyPetSizeMock).toHaveBeenCalledWith(140));
    expect(await screen.findByText('140%')).toBeInTheDocument();
  });

  it('keeps the applied size and announces a persistence failure', async () => {
    const failedSave: PetSizeUpdate = {
      sizePercent: 150,
      saved: false,
      message: '大小已应用，但本地保存失败；重启后将恢复上次保存的大小。'
    };
    applyPetSizeMock.mockResolvedValueOnce(failedSave);
    render(QuickPanel);
    await fireEvent.click(await screen.findByRole('button', { name: '设置' }));
    const slider = screen.getByRole('slider', { name: '桌宠大小' });
    await fireEvent.input(slider, { target: { value: '150' } });
    await fireEvent.change(slider, { target: { value: '150' } });

    expect(await screen.findByRole('alert')).toHaveTextContent('大小已应用，但本地保存失败');
    expect(screen.getByText('150%')).toBeInTheDocument();
  });

  it('closes on Escape and reports readiness without closing on internal pointer events', async () => {
    render(QuickPanel);
    await waitFor(() => expect(notifyQuickPanelReady).toHaveBeenCalledTimes(1));
    await fireEvent.pointerDown(screen.getByRole('button', { name: '设置' }));
    expect(markQuickPanelInternalAction).toHaveBeenCalledTimes(1);
    await fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(closeQuickPanel).toHaveBeenCalledTimes(1));
  });
});
