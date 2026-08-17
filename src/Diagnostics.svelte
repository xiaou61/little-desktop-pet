<script lang="ts">
  import { onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

  import {
    copyDiagnosticsSummary,
    exportDiagnostics,
    fetchDiagnosticEvents,
    fetchDiagnosticsConfig,
    fetchDiagnosticSnapshot,
    fetchRecentCrash,
    openDiagnosticsLogDirectory,
    recordDiagnosticEvent,
    saveDiagnosticsConfig
  } from './lib/api';
  import {
    diagnosticErrorMessage,
    diagnosticLevels,
    filterDiagnosticEvents,
    formatDiagnosticTime,
    levelLabel,
    nextConfig,
    selectLifecycleEvents,
    snapshotStatus,
    toFrontendDiagnosticEvent,
    uniqueValues
  } from './lib/diagnostics';
  import type {
    DiagnosticEvent,
    DiagnosticLevel,
    DiagnosticPage,
    DiagnosticsConfig,
    LastCrash,
    RuntimeSnapshot
  } from './lib/types';

  let events: DiagnosticEvent[] = [];
  let page: DiagnosticPage = { events: [], total: 0, droppedEvents: 0, persistenceDegraded: false };
  let snapshot: RuntimeSnapshot | null = null;
  let crash: LastCrash | null = null;
  let config: DiagnosticsConfig = { developerMode: false, level: 'info' };
  let loading = true;
  let refreshing = false;
  let errorMessage: string | null = null;
  let feedback: string | null = null;
  let exportError: string | null = null;
  let exporting = false;
  let selectedLevel: DiagnosticLevel | '' = '';
  let selectedModule = '';
  let selectedWindow = '';
  let selectedPlugin = '';
  let correlationId = '';
  let selectedFrom = '';
  let selectedTo = '';
  let expandedCorrelation: string | null = null;
  let unsubscribe: UnlistenFn | null = null;

  $: filteredEvents = filterDiagnosticEvents(events, {
    level: selectedLevel || undefined,
    module: selectedModule || undefined,
    windowLabel: selectedWindow || undefined,
    pluginId: selectedPlugin || undefined,
    correlationId: correlationId.trim() || undefined,
    from: selectedFrom || undefined,
    to: selectedTo || undefined
  });
  $: lifecycleEvents = selectLifecycleEvents(events);
  $: modules = uniqueValues(events, 'module');
  $: windows = uniqueValues(events, 'windowLabel');
  $: plugins = uniqueValues(events, 'pluginId');

  onMount(() => {
    void load();
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refreshEvents();
    }, 3000);
    const onError = (event: ErrorEvent) => {
      void reportFrontendError(event.error ?? event.message, 'window.onerror');
    };
    const onRejection = (event: PromiseRejectionEvent) => {
      void reportFrontendError(event.reason, 'unhandledrejection');
    };
    window.addEventListener('error', onError);
    window.addEventListener('unhandledrejection', onRejection);
    void listen<DiagnosticEvent>('diagnostic-event', (event) => {
      events = [event.payload, ...events].slice(0, 500);
    }).then((unlisten) => {
      unsubscribe = unlisten;
    });
    return () => {
      window.clearInterval(timer);
      window.removeEventListener('error', onError);
      window.removeEventListener('unhandledrejection', onRejection);
      unsubscribe?.();
    };
  });

  async function load(): Promise<void> {
    loading = true;
    errorMessage = null;
    try {
      const [nextPage, nextSnapshot, nextConfig, nextCrash] = await Promise.all([
        fetchDiagnosticEvents({ limit: 500 }),
        fetchDiagnosticSnapshot(),
        fetchDiagnosticsConfig(),
        fetchRecentCrash()
      ]);
      page = nextPage;
      events = nextPage.events;
      snapshot = nextSnapshot;
      config = nextConfig;
      crash = nextCrash;
    } catch (error) {
      errorMessage = diagnosticErrorMessage(error, '诊断数据暂时无法读取。');
    } finally {
      loading = false;
    }
  }

  async function refreshEvents(): Promise<void> {
    if (refreshing) return;
    refreshing = true;
    try {
      page = await fetchDiagnosticEvents({ limit: 500 });
      events = page.events;
    } catch (error) {
      errorMessage = diagnosticErrorMessage(error, '日志刷新失败。');
    } finally {
      refreshing = false;
    }
  }

  async function reportFrontendError(error: unknown, source: string): Promise<void> {
    await recordDiagnosticEvent(toFrontendDiagnosticEvent(error, source)).catch(() => undefined);
  }

  async function updateDeveloperMode(enabled: boolean): Promise<void> {
    const previous = config;
    feedback = null;
    try {
      config = await saveDiagnosticsConfig(nextConfig(config, enabled, config.level));
      feedback = enabled ? '开发者模式已开启，详细日志会立即生效。' : '开发者模式已关闭。';
      await refreshEvents();
    } catch (error) {
      config = previous;
      feedback = diagnosticErrorMessage(error, '设置保存失败，已保留原来的模式。');
    }
  }

  async function updateLevel(level: DiagnosticLevel): Promise<void> {
    const previous = config;
    try {
      config = await saveDiagnosticsConfig(nextConfig(config, config.developerMode, level));
      feedback = '日志等级已切换为' + levelLabel(level) + '。';
      await refreshEvents();
    } catch (error) {
      config = previous;
      feedback = diagnosticErrorMessage(error, '日志等级保存失败。');
    }
  }

  async function copySummary(): Promise<void> {
    feedback = null;
    try {
      const summary = await copyDiagnosticsSummary();
      await navigator.clipboard.writeText(summary);
      feedback = '已复制脱敏诊断摘要。';
    } catch (error) {
      feedback = diagnosticErrorMessage(error, '摘要复制失败，请稍后重试。');
    }
  }

  async function exportPackage(): Promise<void> {
    if (exporting) return;
    const destination = window.prompt('请输入诊断包保存路径（.zip）', 'diagnostics.zip')?.trim();
    if (!destination) return;
    exporting = true;
    exportError = null;
    feedback = null;
    try {
      const result = await exportDiagnostics(destination);
      feedback = '诊断包已保存：' + result.path;
    } catch (error) {
      exportError = diagnosticErrorMessage(error, '诊断包导出失败，未发送任何数据。');
    } finally {
      exporting = false;
    }
  }

  async function close(): Promise<void> {
    await getCurrentWebviewWindow().close().catch(() => undefined);
  }

  function toggleCorrelation(id: string | undefined): void {
    if (!id) return;
    expandedCorrelation = expandedCorrelation === id ? null : id;
  }

  function stateText(value: unknown): string {
    if (value === null || value === undefined) return '不可用';
    if (typeof value === 'string') return value;
    try {
      return JSON.stringify(value);
    } catch {
      return '状态不可序列化';
    }
  }
</script>

<svelte:head>
  <title>诊断中心 · 小桌宠</title>
</svelte:head>

<main class="diagnostics" aria-busy={loading || exporting}>
  <header class="diagnostics-header">
    <div>
      <p class="eyebrow">小桌宠 · 本地排错</p>
      <h1>诊断中心</h1>
      <p class="header-status">{snapshot ? snapshotStatus(snapshot) : '正在读取运行状态…'}</p>
    </div>
    <div class="header-actions">
      <button type="button" class="secondary-action" disabled={refreshing} onclick={() => void load()}>刷新</button>
      <button type="button" class="icon-button" aria-label="关闭诊断中心" title="关闭" onclick={() => void close()}>
        <span aria-hidden="true">×</span>
      </button>
    </div>
  </header>

  {#if errorMessage}
    <div class="feedback error" role="alert">
      <span>{errorMessage}</span>
      <button type="button" class="secondary-action" onclick={() => void load()}>重试</button>
    </div>
  {/if}
  {#if feedback}<p class="feedback" role="status">{feedback}</p>{/if}
  {#if exportError}<p class="feedback error" role="alert">{exportError}</p>{/if}

  <section class="diagnostics-toolbar" aria-label="日志筛选">
    <label>最低等级
      <select bind:value={selectedLevel}>
        <option value="">全部</option>
        {#each diagnosticLevels as level}<option value={level}>{levelLabel(level)}</option>{/each}
      </select>
    </label>
    <label>模块
      <select bind:value={selectedModule}><option value="">全部模块</option>{#each modules as module}<option value={module}>{module}</option>{/each}</select>
    </label>
    <label>窗口
      <select bind:value={selectedWindow}><option value="">全部窗口</option>{#each windows as window}<option value={window}>{window}</option>{/each}</select>
    </label>
    <label>插件
      <select bind:value={selectedPlugin}><option value="">全部插件</option>{#each plugins as plugin}<option value={plugin}>{plugin}</option>{/each}</select>
    </label>
    <label class="correlation-filter">关联 ID
      <input bind:value={correlationId} placeholder="输入关联 ID" />
    </label>
    <label>开始时间
      <input type="datetime-local" bind:value={selectedFrom} />
    </label>
    <label>结束时间
      <input type="datetime-local" bind:value={selectedTo} />
    </label>
    <button type="button" class="link-action" onclick={() => {
      selectedLevel = ''; selectedModule = ''; selectedWindow = ''; selectedPlugin = ''; correlationId = ''; selectedFrom = ''; selectedTo = '';
    }}>清除筛选</button>
  </section>

  <div class="diagnostics-grid">
    <section class="panel logs-panel" aria-labelledby="logs-title">
      <div class="section-heading">
        <div><h2 id="logs-title">最近日志</h2><span>{filteredEvents.length} / {events.length}</span></div>
        {#if page.persistenceDegraded}<span class="warning-badge">持久化降级</span>{/if}
      </div>
      {#if loading}
        <p class="state-panel">正在读取日志…</p>
      {:else if filteredEvents.length === 0}
        <p class="state-panel">当前筛选条件下没有日志。</p>
      {:else}
        <ul class="event-list" aria-label="结构化诊断日志">
          {#each filteredEvents as event (event.timestamp + event.module + event.event)}
            <li class:has-correlation={Boolean(event.correlationId)} data-level={event.level}>
              <div class="event-row">
                <time datetime={event.timestamp}>{formatDiagnosticTime(event.timestamp)}</time>
                <span class="level-badge">{levelLabel(event.level)}</span>
                <strong>{event.module}</strong>
                <span class="event-name">{event.event}</span>
                {#if event.correlationId}
                  <button type="button" class="correlation-badge" aria-expanded={expandedCorrelation === event.correlationId} onclick={() => toggleCorrelation(event.correlationId)}>
                    {event.correlationId}
                  </button>
                {/if}
              </div>
              <p>{event.message}</p>
              {#if event.errorCode || event.windowLabel || event.pluginId || event.durationMs !== undefined}
                <div class="event-meta">
                  {#if event.errorCode}<span>错误码 {event.errorCode}</span>{/if}
                  {#if event.windowLabel}<span>窗口 {event.windowLabel}</span>{/if}
                  {#if event.pluginId}<span>插件 {event.pluginId}</span>{/if}
                  {#if event.durationMs !== undefined}<span>{event.durationMs} ms</span>{/if}
                </div>
              {/if}
              {#if event.context && Object.keys(event.context).length > 0}<details><summary>上下文</summary><pre>{JSON.stringify(event.context, null, 2)}</pre></details>{/if}
              {#if expandedCorrelation && event.correlationId === expandedCorrelation}<div class="correlation-note">已按关联 ID 展开此链路，其他事件可通过上方筛选定位。</div>{/if}
            </li>
          {/each}
        </ul>
      {/if}
      <footer class="panel-footer">
        <span>内存 ring 500 条 · 丢弃 {page.droppedEvents} 条</span>
        <button type="button" class="link-action" onclick={() => void openDiagnosticsLogDirectory()}>打开日志目录</button>
      </footer>
    </section>

    <aside class="side-column">
      <section class="panel settings-panel" aria-labelledby="settings-title">
        <div class="section-heading"><h2 id="settings-title">开发者设置</h2><span>{config.developerMode ? '已开启' : '普通模式'}</span></div>
        <label class="toggle-row"><span>开发者模式</span><input type="checkbox" checked={config.developerMode} onchange={(event) => void updateDeveloperMode((event.currentTarget as HTMLInputElement).checked)} /></label>
        <label>日志等级
          <select value={config.level} onchange={(event) => void updateLevel((event.currentTarget as HTMLSelectElement).value as DiagnosticLevel)}>
            {#each diagnosticLevels as level}<option value={level}>{levelLabel(level)}</option>{/each}
          </select>
        </label>
        <p class="muted">发布版默认关闭；桌宠原生窗口不提供 WebView DevTools。</p>
      </section>

      <section class="panel actions-panel" aria-labelledby="actions-title">
        <div class="section-heading"><h2 id="actions-title">诊断材料</h2></div>
        <button type="button" class="primary-action" onclick={() => void copySummary()}>复制脱敏摘要</button>
        <button type="button" class="secondary-action" disabled={exporting} onclick={() => void exportPackage()}>{exporting ? '正在导出…' : '导出诊断包'}</button>
        <p class="muted">导出前会列出固定文件清单，内容只保存在你选择的位置。</p>
      </section>

      <section class="panel crash-panel" aria-labelledby="crash-title">
        <div class="section-heading"><h2 id="crash-title">最近异常</h2><span>{crash ? '已记录' : '无记录'}</span></div>
        {#if crash}
          <p><strong>{crash.source}</strong> · {formatDiagnosticTime(crash.timestamp)}</p>
          <p class="crash-message">{crash.message}</p>
          {#if crash.backtrace}<details><summary>查看脱敏 backtrace</summary><pre>{crash.backtrace}</pre></details>{/if}
        {:else}<p class="muted">当前没有 panic 或异常退出摘要。</p>{/if}
      </section>
    </aside>
  </div>

  <section class="panel timeline-panel" aria-labelledby="timeline-title">
    <div class="section-heading"><div><h2 id="timeline-title">运行状态与生命周期</h2><span>{snapshot?.buildMode ?? '—'} · {snapshot?.appVersion ?? '—'}</span></div></div>
    {#if snapshot}
      <div class="snapshot-grid">
        <article><span>桌宠</span><strong>{snapshot.pet.available ? stateText(snapshot.pet.state) : '不可用'}</strong></article>
        <article><span>快捷面板</span><strong>{snapshot.quickPanel.available ? stateText(snapshot.quickPanel.state) : '不可用'}</strong></article>
        <article><span>采集器</span><strong>{snapshot.collector.available ? stateText(snapshot.collector.state) : '不可用'}</strong></article>
        <article><span>插件</span><strong>{snapshot.plugins.available ? '可读取' : '不可用'}</strong></article>
      </div>
      {#if snapshot.webviewLabels.length > 0}<p class="muted">已创建 WebView：{snapshot.webviewLabels.join('、')}</p>{/if}
    {:else}<p class="state-panel">状态快照暂时不可用。</p>{/if}
    <div class="lifecycle-block">
      <h3>生命周期时间线</h3>
      {#if lifecycleEvents.length === 0}
        <p class="muted">当前会话尚无生命周期事件。</p>
      {:else}
        <ol class="lifecycle-list">
          {#each lifecycleEvents as event (event.timestamp + event.event)}
            <li>
              <time datetime={event.timestamp}>{formatDiagnosticTime(event.timestamp)}</time>
              <strong>{event.event}</strong>
              <span>{event.message}</span>
            </li>
          {/each}
        </ol>
      {/if}
    </div>
  </section>
</main>
