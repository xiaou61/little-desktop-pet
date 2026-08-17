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
