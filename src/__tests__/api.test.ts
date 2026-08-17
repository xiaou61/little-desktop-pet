import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invoke } from '@tauri-apps/api/core';

import { applyPetSkin, fetchCurrentPetSkin, fetchPetSkins } from '../lib/api';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn()
}));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  vi.clearAllMocks();
});

describe('pet skin API', () => {
  it('reads the local manifest and current state without collector calls', async () => {
    invokeMock
      .mockResolvedValueOnce([
        {
          id: 'simple-cloud',
          displayName: '简洁云朵',
          thumbnailDataUrl: 'data:image/png;base64,cloud',
          available: true
        }
      ])
      .mockResolvedValueOnce({ skinId: 'simple-cloud' });

    await expect(fetchPetSkins()).resolves.toHaveLength(1);
    await expect(fetchCurrentPetSkin()).resolves.toEqual({ skinId: 'simple-cloud' });
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'get_pet_skins');
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'get_current_pet_skin');
    expect(invokeMock).not.toHaveBeenCalledWith('get_daily_usage', expect.anything());
  });

  it('passes only the stable skin ID to the native setting command', async () => {
    invokeMock.mockResolvedValueOnce({
      skinId: 'orange-dragon',
      saved: true,
      message: null
    });

    await expect(applyPetSkin('orange-dragon')).resolves.toMatchObject({
      skinId: 'orange-dragon',
      saved: true
    });
    expect(invokeMock).toHaveBeenCalledWith('set_pet_skin', { skinId: 'orange-dragon' });
  });
});
