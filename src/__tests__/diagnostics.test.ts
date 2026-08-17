import { describe, expect, it } from 'vitest';

import {
  diagnosticErrorMessage,
  filterDiagnosticEvents,
  levelAllows,
  nextConfig,
  selectLifecycleEvents,
  snapshotStatus,
  toFrontendDiagnosticEvent
} from '../lib/diagnostics';
import type { DiagnosticEvent, DiagnosticsConfig, RuntimeSnapshot } from '../lib/types';

const events: DiagnosticEvent[] = [
  { timestamp: '2026-08-17T00:00:00Z', level: 'info', module: 'pet', event: 'shown', message: 'ok', correlationId: 'c1' },
  { timestamp: '2026-08-17T00:00:01Z', level: 'error', module: 'panel', event: 'failed', message: 'bad', correlationId: 'c1' },
  { timestamp: '2026-08-17T00:00:02Z', level: 'debug', module: 'collector', event: 'sample', message: 'ignored' }
];

describe('diagnostics helpers', () => {
  it('filters by minimum level, module and correlation', () => {
    expect(filterDiagnosticEvents(events, { level: 'warn' })).toHaveLength(1);
    expect(filterDiagnosticEvents(events, { module: 'panel', correlationId: 'c1' })[0].event).toBe('failed');
    expect(levelAllows('info', events[2])).toBe(false);
  });

  it('filters by an inclusive time range and ignores invalid boundaries', () => {
    expect(filterDiagnosticEvents(events, {
      from: '2026-08-17T00:00:01Z',
      to: '2026-08-17T00:00:02Z'
    })).toHaveLength(2);
    expect(filterDiagnosticEvents(events, { from: 'invalid' })).toEqual(events);
  });

  it('selects a bounded lifecycle timeline', () => {
    const lifecycle = events.map((event, index) => ({ ...event, module: 'lifecycle', event: `step-${index}` }));
    expect(selectLifecycleEvents([...lifecycle, ...events], 2).map((event) => event.event)).toEqual(['step-0', 'step-1']);
  });

  it('keeps errors bounded and converts unknown frontend failures', () => {
    expect(diagnosticErrorMessage({ message: 'ipc failed' }, 'fallback')).toBe('ipc failed');
    const event = toFrontendDiagnosticEvent(new Error('boom'), 'window.onerror');
    expect(event.level).toBe('error');
    expect(event.context?.source).toBe('window.onerror');
  });

  it('reports partial state degradation and updates config immutably', () => {
    const snapshot: RuntimeSnapshot = {
      appVersion: '0.1.0',
      buildMode: 'debug',
      developerMode: false,
      pet: { available: true, state: {}, error: null },
      quickPanel: { available: false, state: null, error: 'failed' },
      collector: { available: true, state: {}, error: null },
      plugins: { available: true, state: [], error: null },
      webviewLabels: [],
      persistenceDegraded: false,
      droppedEvents: 0
    };
    expect(snapshotStatus(snapshot)).toContain('不可用');
    const config: DiagnosticsConfig = { developerMode: false, level: 'info' };
    expect(nextConfig(config, true, 'debug')).toEqual({ developerMode: true, level: 'debug' });
    expect(config).toEqual({ developerMode: false, level: 'info' });
  });
});
