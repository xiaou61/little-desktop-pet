import { invoke } from '@tauri-apps/api/core';

import type {
  DailyUsageSummary,
  PetSkinOption,
  PetSkinStatus,
  PetSkinUpdate,
  PetSizeStatus,
  PetSizeUpdate,
  QuickPanelEnvironment,
  TrackerStatus
} from './types';

export function fetchDailyUsage(date: string): Promise<DailyUsageSummary> {
  return invoke<DailyUsageSummary>('get_daily_usage', { date });
}

export function fetchTrackerStatus(): Promise<TrackerStatus> {
  return invoke<TrackerStatus>('get_tracker_status');
}

export async function closeDashboard(): Promise<void> {
  const { getCurrentWebviewWindow } = await import('@tauri-apps/api/webviewWindow');
  await getCurrentWebviewWindow().close();
}

export function fetchPetSize(): Promise<PetSizeStatus> {
  return invoke<PetSizeStatus>('get_pet_size');
}

export function applyPetSize(sizePercent: number): Promise<PetSizeUpdate> {
  return invoke<PetSizeUpdate>('set_pet_size', { sizePercent });
}

export function previewPetSize(sizePercent: number): Promise<PetSizeStatus> {
  return invoke<PetSizeStatus>('preview_pet_size', { sizePercent });
}

export function fetchPetSkins(): Promise<PetSkinOption[]> {
  return invoke<PetSkinOption[]>('get_pet_skins');
}

export function fetchCurrentPetSkin(): Promise<PetSkinStatus> {
  return invoke<PetSkinStatus>('get_current_pet_skin');
}

export function applyPetSkin(skinId: string): Promise<PetSkinUpdate> {
  return invoke<PetSkinUpdate>('set_pet_skin', { skinId });
}

export function fetchQuickPanelEnvironment(): Promise<QuickPanelEnvironment> {
  return invoke<QuickPanelEnvironment>('get_quick_panel_environment');
}

export function notifyQuickPanelReady(): Promise<void> {
  return invoke('quick_panel_ready');
}

export function markQuickPanelInternalAction(): Promise<void> {
  return invoke('quick_panel_internal_action');
}

export function closeQuickPanel(): Promise<void> {
  return invoke('close_quick_panel');
}

export function openFullStatistics(): Promise<void> {
  return invoke('open_full_statistics');
}
