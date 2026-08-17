import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import PluginManager from '../PluginManager.svelte';
import {
  enablePlugin,
  fetchPluginContributions,
  fetchPluginDirectory,
  installOfficialPlugin,
  installPluginPackage,
  previewPluginPackage,
  uninstallPlugin
} from '../lib/api';
import type { PluginDirectory, PluginSummary } from '../lib/types';

vi.mock('../lib/api', () => ({
  disablePlugin: vi.fn(),
  enablePlugin: vi.fn(),
  fetchPluginContributions: vi.fn(),
  fetchPluginDirectory: vi.fn(),
  installOfficialPlugin: vi.fn(),
  installPluginPackage: vi.fn(),
  previewPluginPackage: vi.fn(),
  uninstallPlugin: vi.fn()
}));

vi.mock('@tauri-apps/api/webviewWindow', () => ({
  getCurrentWebviewWindow: vi.fn(() => ({ close: vi.fn().mockResolvedValue(undefined) }))
}));

const fetchDirectoryMock = vi.mocked(fetchPluginDirectory);
const fetchContributionsMock = vi.mocked(fetchPluginContributions);
const previewMock = vi.mocked(previewPluginPackage);
const installMock = vi.mocked(installPluginPackage);
const enableMock = vi.mocked(enablePlugin);

const cloud: PluginSummary = {
  id: 'simple-cloud',
  displayName: '简洁云朵',
  version: '1.0.0',
  kind: 'skin',
  source: 'builtIn',
  state: 'enabled',
  permissions: [],
  contributions: ['skins'],
  lastError: null,
  protected: true,
  installed: true
};

function directory(overrides: Partial<PluginDirectory> = {}): PluginDirectory {
  return {
    installed: [cloud],
    available: [
      {
        id: 'simple-cloud',
        displayName: '简洁云朵',
        version: '1.0.0',
        kind: 'skin',
        source: 'builtIn',
        thumbnailDataUrl: 'data:image/png;base64,cloud',
        contributions: ['skins'],
        permissions: [],
        installed: true
      },
      {
        id: 'orange-dragon',
        displayName: '橙色小龙',
        version: '1.0.0',
        kind: 'skin',
        source: 'officialDirectory',
        thumbnailDataUrl: 'data:image/png;base64,dragon',
        contributions: ['skins'],
        permissions: [],
        installed: false
      }
    ],
    ...overrides
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  fetchDirectoryMock.mockResolvedValue(directory());
  fetchContributionsMock.mockResolvedValue([]);
  enableMock.mockResolvedValue({ ...cloud, state: 'enabled' });
});

describe('plugin manager', () => {
  it('shows only the default skin installed and keeps official alternatives in the offline catalog', async () => {
    render(PluginManager);

    expect(await screen.findByRole('heading', { name: '插件管理' })).toBeInTheDocument();
    await waitFor(() => expect(screen.getAllByText('简洁云朵')).toHaveLength(2));
    expect(screen.getByText('橙色小龙')).toBeInTheDocument();
    expect(screen.getByText('已启用')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '安装' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '禁用' })).toBeDisabled();
  });

  it('previews a local package and installs only after confirmation', async () => {
    const preview: PluginSummary = {
      ...cloud,
      id: 'example-skin',
      displayName: '示例皮肤',
      source: 'localImport',
      state: 'discovered',
      protected: false,
      installed: false
    };
    previewMock.mockResolvedValue(preview);
    installMock.mockResolvedValue({ ...preview, state: 'installed', installed: true });
    render(PluginManager);

    const file = new File(['petpack'], 'demo-skin.petpack', { type: 'application/zip' });
    Object.defineProperty(file, 'path', { value: 'C:\\demo-skin.petpack' });
    await fireEvent.change(screen.getByLabelText('选择文件'), { target: { files: [file] } });

    expect(await screen.findByText('示例皮肤')).toBeInTheDocument();
    expect(installMock).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: '确认安装' }));
    await waitFor(() => expect(installMock).toHaveBeenCalledWith('C:\\demo-skin.petpack'));
  });

  it('reports offline directory errors without exposing native paths', async () => {
    fetchDirectoryMock.mockRejectedValueOnce(new Error('C:\\private\\plugin-registry.json'));
    render(PluginManager);

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('插件目录暂时无法读取');
    expect(alert).not.toHaveTextContent('private');
  });
});
