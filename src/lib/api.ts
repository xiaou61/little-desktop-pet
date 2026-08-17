import { invoke } from '@tauri-apps/api/core';

import type {
  DailyUsageSummary,
  DiagnosticEvent,
  DiagnosticPage,
  DiagnosticQuery,
  DiagnosticsConfig,
  DiagnosticsExportResult,
  LastCrash,
  PetSkinOption,
  PetSkinStatus,
  PetSkinUpdate,
  PetSizeStatus,
  PetSizeUpdate,
  PluginContribution,
  PluginDirectory,
  PluginSummary,
  QuickPanelEnvironment,
  RuntimeSnapshot,
  TrackerStatus
} from './types';

async function invokeWithDiagnostics<T>(
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  try {
    return args === undefined ? await invoke<T>(command) : await invoke<T>(command, args);
  } catch (error) {
    const publicError =
      typeof error === 'object' && error !== null
        ? (error as { code?: unknown; message?: unknown })
        : {};
    const event: DiagnosticEvent = {
      timestamp: new Date().toISOString(),
      level: 'error',
      module: 'tauri-ipc',
      event: 'command-failed',
      message: String(publicError.message ?? ('IPC command ' + command + ' failed')).slice(0, 2000),
      errorCode: publicError.code ? String(publicError.code).slice(0, 80) : undefined,
      context: { source: command }
    };
    await invoke('record_diagnostic_event', { event }).catch(() => undefined);
    throw error;
  }
}

export function fetchDailyUsage(date: string): Promise<DailyUsageSummary> {
  return invokeWithDiagnostics<DailyUsageSummary>('get_daily_usage', { date });
}

export function fetchTrackerStatus(): Promise<TrackerStatus> {
  return invokeWithDiagnostics<TrackerStatus>('get_tracker_status');
}

export async function closeDashboard(): Promise<void> {
  const { getCurrentWebviewWindow } = await import('@tauri-apps/api/webviewWindow');
  await getCurrentWebviewWindow().close();
}

export function fetchPetSize(): Promise<PetSizeStatus> {
  return invokeWithDiagnostics<PetSizeStatus>('get_pet_size');
}

export function applyPetSize(sizePercent: number): Promise<PetSizeUpdate> {
  return invokeWithDiagnostics<PetSizeUpdate>('set_pet_size', { sizePercent });
}

export function previewPetSize(sizePercent: number): Promise<PetSizeStatus> {
  return invokeWithDiagnostics<PetSizeStatus>('preview_pet_size', { sizePercent });
}

export function fetchPetSkins(): Promise<PetSkinOption[]> {
  return invokeWithDiagnostics<PetSkinOption[]>('get_pet_skins');
}

export function fetchCurrentPetSkin(): Promise<PetSkinStatus> {
  return invokeWithDiagnostics<PetSkinStatus>('get_current_pet_skin');
}

export function applyPetSkin(skinId: string): Promise<PetSkinUpdate> {
  return invokeWithDiagnostics<PetSkinUpdate>('set_pet_skin', { skinId });
}

export function fetchPluginDirectory(): Promise<PluginDirectory> {
  return invokeWithDiagnostics<PluginDirectory>('get_plugin_directory');
}

export function previewPluginPackage(path: string): Promise<PluginSummary> {
  return invokeWithDiagnostics<PluginSummary>('preview_plugin_package', { path });
}

export function installPluginPackage(path: string): Promise<PluginSummary> {
  return invokeWithDiagnostics<PluginSummary>('install_plugin_package', { path });
}

export function installOfficialPlugin(pluginId: string): Promise<PluginSummary> {
  return invokeWithDiagnostics<PluginSummary>('install_official_plugin', { pluginId });
}

export function enablePlugin(pluginId: string): Promise<PluginSummary> {
  return invokeWithDiagnostics<PluginSummary>('enable_plugin', { pluginId });
}

export function disablePlugin(pluginId: string): Promise<PluginSummary> {
  return invokeWithDiagnostics<PluginSummary>('disable_plugin', { pluginId });
}

export function uninstallPlugin(pluginId: string): Promise<void> {
  return invokeWithDiagnostics<void>('uninstall_plugin', { pluginId });
}

export function fetchPluginContributions(pluginId: string): Promise<PluginContribution[]> {
  return invokeWithDiagnostics<PluginContribution[]>('get_plugin_contributions', { pluginId });
}

export function executePluginAction(
  pluginId: string,
  contributionId: string,
  actionId: string
): Promise<void> {
  return invokeWithDiagnostics<void>('execute_plugin_action', { pluginId, contributionId, actionId });
}

export function openPluginManager(): Promise<void> {
  return invokeWithDiagnostics<void>('open_plugin_manager');
}

export function fetchQuickPanelEnvironment(): Promise<QuickPanelEnvironment> {
  return invokeWithDiagnostics<QuickPanelEnvironment>('get_quick_panel_environment');
}

export function notifyQuickPanelReady(): Promise<void> {
  return invokeWithDiagnostics<void>('quick_panel_ready');
}

export function markQuickPanelInternalAction(): Promise<void> {
  return invokeWithDiagnostics<void>('quick_panel_internal_action');
}

export function closeQuickPanel(): Promise<void> {
  return invokeWithDiagnostics<void>('close_quick_panel');
}

export function openFullStatistics(): Promise<void> {
  return invokeWithDiagnostics<void>('open_full_statistics');
}

export function fetchDiagnosticEvents(query: DiagnosticQuery = {}): Promise<DiagnosticPage> {
  return invokeWithDiagnostics<DiagnosticPage>('get_diagnostic_events', { query });
}

export function fetchRecentDiagnosticErrors(): Promise<DiagnosticPage> {
  return invokeWithDiagnostics<DiagnosticPage>('get_recent_errors');
}

export function fetchDiagnosticsConfig(): Promise<DiagnosticsConfig> {
  return invokeWithDiagnostics<DiagnosticsConfig>('get_diagnostics_config');
}

export function saveDiagnosticsConfig(config: DiagnosticsConfig): Promise<DiagnosticsConfig> {
  return invokeWithDiagnostics<DiagnosticsConfig>('set_diagnostics_config', { config });
}

export function fetchDiagnosticSnapshot(): Promise<RuntimeSnapshot> {
  return invokeWithDiagnostics<RuntimeSnapshot>('get_diagnostic_snapshot');
}

export function fetchRecentCrash(): Promise<LastCrash | null> {
  return invokeWithDiagnostics<LastCrash | null>('get_recent_crash');
}

export function recordDiagnosticEvent(event: DiagnosticEvent): Promise<void> {
  return invoke('record_diagnostic_event', { event });
}

export function fetchDiagnosticSummary(): Promise<string> {
  return invokeWithDiagnostics<string>('copy_diagnostics_summary');
}

export function copyDiagnosticsSummary(): Promise<string> {
  return fetchDiagnosticSummary();
}

export function exportDiagnostics(destination: string): Promise<DiagnosticsExportResult> {
  return invokeWithDiagnostics<DiagnosticsExportResult>('export_diagnostics', { destination });
}

export function openDiagnosticsLogDirectory(): Promise<void> {
  return invokeWithDiagnostics<void>('open_diagnostics_log_directory');
}

export function openDiagnosticsCenter(): Promise<void> {
  return invokeWithDiagnostics<void>('open_diagnostics_center');
}
