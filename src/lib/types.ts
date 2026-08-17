export type TrackerState = 'recording' | 'idle' | 'unavailable' | 'error';

export interface DailyApplicationUsage {
  displayName: string;
  executableName: string;
  activeMs: number;
  share: number;
}

export interface DailyUsageSummary {
  date: string;
  trackerState: TrackerState;
  totalActiveMs: number;
  applications: DailyApplicationUsage[];
}

export interface TrackerStatus {
  state: TrackerState;
  message: string | null;
}

export interface InvokeError {
  code: string;
  message: string;
}

export interface PetSizeStatus {
  sizePercent: number;
}

export interface PetSizeUpdate extends PetSizeStatus {
  saved: boolean;
  message: string | null;
}

export interface PetSkinOption {
  id: string;
  displayName: string;
  thumbnailDataUrl: string;
  available: boolean;
}

export interface PetSkinStatus {
  skinId: string;
}

export interface PetSkinUpdate extends PetSkinStatus {
  saved: boolean;
  message: string | null;
}

export interface QuickPanelEnvironment {
  glassAvailable: boolean;
  highContrast: boolean;
  reduceMotion: boolean;
  lastError: string | null;
}

export type PluginState =
  | 'discovered'
  | 'installed'
  | 'enabled'
  | 'disabled'
  | 'broken'
  | 'removed';

export type PluginSource = 'builtIn' | 'officialDirectory' | 'localImport';
export type PluginContributionType = 'skins' | 'panelCards' | 'settings' | 'menus';

export interface PluginSummary {
  id: string;
  displayName: string;
  version: string;
  kind: string;
  source: PluginSource;
  state: PluginState;
  permissions: string[];
  contributions: PluginContributionType[];
  lastError: string | null;
  protected: boolean;
  installed: boolean;
}

export interface PluginCatalogEntry {
  id: string;
  displayName: string;
  version: string;
  kind: string;
  source: PluginSource;
  thumbnailDataUrl: string | null;
  contributions: PluginContributionType[];
  permissions: string[];
  installed: boolean;
}

export interface PluginDirectory {
  installed: PluginSummary[];
  available: PluginCatalogEntry[];
}

export interface PluginAction {
  id: string;
  action: string;
  label: string | null;
}

export interface PluginContribution {
  id: string;
  type: PluginContributionType;
  resource: string | null;
  thumbnail: string | null;
  width: number | null;
  height: number | null;
  label: string | null;
  actions: PluginAction[];
}

export type DiagnosticLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error';

export interface DiagnosticEvent {
  timestamp: string;
  level: DiagnosticLevel;
  module: string;
  event: string;
  message: string;
  errorCode?: string;
  windowLabel?: string;
  pluginId?: string;
  correlationId?: string;
  durationMs?: number;
  context?: Record<string, string | number | boolean | null>;
}

export interface DiagnosticQuery {
  level?: DiagnosticLevel;
  module?: string;
  windowLabel?: string;
  pluginId?: string;
  correlationId?: string;
  from?: string;
  to?: string;
  offset?: number;
  limit?: number;
}

export interface DiagnosticPage {
  events: DiagnosticEvent[];
  total: number;
  droppedEvents: number;
  persistenceDegraded: boolean;
}

export interface DiagnosticsConfig {
  developerMode: boolean;
  level: DiagnosticLevel;
}

export interface DiagnosticComponentSnapshot {
  available: boolean;
  state: unknown | null;
  error: string | null;
}

export interface RuntimeSnapshot {
  appVersion: string;
  buildMode: string;
  developerMode: boolean;
  pet: DiagnosticComponentSnapshot;
  quickPanel: DiagnosticComponentSnapshot;
  collector: DiagnosticComponentSnapshot;
  plugins: DiagnosticComponentSnapshot;
  webviewLabels: string[];
  persistenceDegraded: boolean;
  droppedEvents: number;
}

export interface LastCrash {
  timestamp: string;
  source: string;
  message: string;
  backtrace: string | null;
}

export interface DiagnosticsExportResult {
  path: string;
  files: string[];
  lastCrashIncluded: boolean;
}
