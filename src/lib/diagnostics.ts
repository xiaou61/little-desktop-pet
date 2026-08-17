import type {
  DiagnosticEvent,
  DiagnosticLevel,
  DiagnosticQuery,
  DiagnosticsConfig,
  RuntimeSnapshot
} from './types';

export const diagnosticLevels: DiagnosticLevel[] = ['trace', 'debug', 'info', 'warn', 'error'];

const levelRank: Record<DiagnosticLevel, number> = {
  trace: 0,
  debug: 1,
  info: 2,
  warn: 3,
  error: 4
};

export function levelLabel(level: DiagnosticLevel): string {
  return {
    trace: '跟踪',
    debug: '调试',
    info: '信息',
    warn: '警告',
    error: '错误'
  }[level];
}

export function levelAllows(minimum: DiagnosticLevel | undefined, event: DiagnosticEvent): boolean {
  return minimum === undefined || levelRank[event.level] >= levelRank[minimum];
}

export function filterDiagnosticEvents(
  events: DiagnosticEvent[],
  query: Pick<DiagnosticQuery, 'level' | 'module' | 'windowLabel' | 'pluginId' | 'correlationId' | 'from' | 'to'>
): DiagnosticEvent[] {
  const from = query.from ? Date.parse(query.from) : Number.NaN;
  const to = query.to ? Date.parse(query.to) : Number.NaN;
  return events.filter((event) => {
    const timestamp = Date.parse(event.timestamp);
    const afterStart = Number.isNaN(from) || (!Number.isNaN(timestamp) && timestamp >= from);
    const beforeEnd = Number.isNaN(to) || (!Number.isNaN(timestamp) && timestamp <= to);
    return levelAllows(query.level, event) &&
      (!query.module || event.module === query.module) &&
      (!query.windowLabel || event.windowLabel === query.windowLabel) &&
      (!query.pluginId || event.pluginId === query.pluginId) &&
      (!query.correlationId || event.correlationId === query.correlationId) &&
      afterStart &&
      beforeEnd;
  });
}

export function selectLifecycleEvents(events: DiagnosticEvent[], limit = 8): DiagnosticEvent[] {
  return events.filter((event) => event.module === 'lifecycle').slice(0, limit);
}

export function uniqueValues(events: DiagnosticEvent[], field: 'module' | 'windowLabel' | 'pluginId'): string[] {
  return [...new Set(events.map((event) => event[field]).filter((value): value is string => Boolean(value)))].sort();
}

export function formatDiagnosticTime(timestamp: string): string {
  const date = new Date(timestamp);
  return Number.isNaN(date.valueOf()) ? timestamp : date.toLocaleString('zh-CN', { hour12: false });
}

export function diagnosticErrorMessage(error: unknown, fallback: string): string {
  if (typeof error === 'object' && error !== null && 'message' in error) {
    const message = String((error as { message?: unknown }).message ?? '');
    if (message && message.length <= 180) return message;
  }
  return fallback;
}

export function toFrontendDiagnosticEvent(error: unknown, source: string): DiagnosticEvent {
  const message = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
  return {
    timestamp: new Date().toISOString(),
    level: 'error',
    module: source,
    event: 'frontend-error',
    message: message.slice(0, 2000),
    context: { source }
  };
}

export function snapshotStatus(snapshot: RuntimeSnapshot): string {
  const unavailable = [snapshot.pet, snapshot.quickPanel, snapshot.collector, snapshot.plugins].filter(
    (component) => !component.available
  ).length;
  if (unavailable > 0) return `${unavailable} 个状态源不可用`;
  if (snapshot.persistenceDegraded) return '日志持久化降级';
  return '运行正常';
}

export function nextConfig(config: DiagnosticsConfig, developerMode: boolean, level: DiagnosticLevel): DiagnosticsConfig {
  return { ...config, developerMode, level };
}

export function installGlobalDiagnosticHandlers(source: string): () => void {
  const report = async (error: unknown, event: string) => {
    try {
      const { recordDiagnosticEvent } = await import('./api');
      const diagnostic = toFrontendDiagnosticEvent(error, source);
      diagnostic.event = event;
      await recordDiagnosticEvent(diagnostic);
    } catch {
      // Diagnostics must never create another unhandled rejection.
    }
  };
  const onError = (event: ErrorEvent) => void report(event.error ?? event.message, 'frontend-unhandled-error');
  const onRejection = (event: PromiseRejectionEvent) => void report(event.reason, 'frontend-unhandled-rejection');
  window.addEventListener('error', onError);
  window.addEventListener('unhandledrejection', onRejection);
  return () => {
    window.removeEventListener('error', onError);
    window.removeEventListener('unhandledrejection', onRejection);
  };
}
