<script lang="ts">
  import { onMount } from 'svelte';

  import { closeDashboard, fetchDailyUsage, fetchTrackerStatus } from './lib/api';
  import {
    addLocalDays,
    canNavigateForward,
    formatCalendarDate,
    nextLocalDate,
    toLocalDateString
  } from './lib/date';
  import { formatDuration, formatPercentage } from './lib/format';
  import type {
    DailyApplicationUsage,
    DailyUsageSummary,
    TrackerState,
    TrackerStatus
  } from './lib/types';

  const REFRESH_INTERVAL_MS = 5_000;
  const today = toLocalDateString();
  let selectedDate = today;
  let summary: DailyUsageSummary | null = null;
  let status: TrackerStatus | null = null;
  let loading = true;
  let errorMessage: string | null = null;
  let requestSequence = 0;

  $: applications = sortApplications(summary?.applications ?? []);
  $: trackerState = summary?.trackerState ?? status?.state ?? 'unavailable';
  $: selectedDateLabel = formatCalendarDate(selectedDate, today);
  $: forwardDisabled = !canNavigateForward(selectedDate, today);

  onMount(() => {
    void refresh(true);
    const refreshTimer = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void refresh(false);
      }
    }, REFRESH_INTERVAL_MS);
    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') {
        void refresh(false);
      }
    };
    document.addEventListener('visibilitychange', onVisibilityChange);

    return () => {
      window.clearInterval(refreshTimer);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  });

  async function refresh(showLoading: boolean): Promise<void> {
    const sequence = ++requestSequence;
    if (showLoading) {
      loading = true;
    }
    try {
      const nextSummary = await fetchDailyUsage(selectedDate);
      if (sequence !== requestSequence) return;
      summary = nextSummary;
      status = { state: nextSummary.trackerState, message: null };
      errorMessage = null;
    } catch {
      if (sequence !== requestSequence) return;
      summary = null;
      try {
        status = await fetchTrackerStatus();
      } catch {
        status = { state: 'error', message: null };
      }
      errorMessage = status?.message ?? '本地使用记录暂时无法读取。';
    } finally {
      if (sequence === requestSequence) {
        loading = false;
      }
    }
  }

  function changeDate(nextDate: string): void {
    if (nextDate === selectedDate) return;
    selectedDate = nextDate;
    summary = null;
    errorMessage = null;
    void refresh(true);
  }

  function sortApplications(items: DailyApplicationUsage[]): DailyApplicationUsage[] {
    return [...items].sort(
      (left, right) =>
        right.activeMs - left.activeMs ||
        left.displayName.localeCompare(right.displayName, 'zh-CN') ||
        left.executableName.localeCompare(right.executableName, 'en')
    );
  }

  function stateLabel(state: TrackerState): string {
    return {
      recording: '正在记录',
      idle: '空闲暂停',
      unavailable: '暂不可记录',
      error: '保存异常'
    }[state];
  }
</script>

<svelte:head>
  <title>使用统计</title>
</svelte:head>

<main class="dashboard" aria-busy={loading}>
  <header class="titlebar">
    <div>
      <p class="eyebrow">小桌宠 · 使用统计</p>
      <h1>{selectedDateLabel}</h1>
    </div>
    <button
      class="icon-button"
      type="button"
      aria-label="关闭窗口"
      title="关闭窗口"
      onclick={() => void closeDashboard()}
    >
      <span aria-hidden="true">×</span>
    </button>
  </header>

  <nav class="date-toolbar" aria-label="日期导航">
    <button
      class="icon-button"
      type="button"
      aria-label="前一天"
      title="前一天"
      onclick={() => changeDate(addLocalDays(selectedDate, -1))}
    >
      <span aria-hidden="true">‹</span>
    </button>
    <button
      class="today-button"
      type="button"
      disabled={selectedDate === today}
      onclick={() => changeDate(today)}
    >
      回到今天
    </button>
    <button
      class="icon-button"
      type="button"
      aria-label="后一天"
      title="后一天"
      disabled={forwardDisabled}
      onclick={() => changeDate(nextLocalDate(selectedDate, today))}
    >
      <span aria-hidden="true">›</span>
    </button>
  </nav>

  <section class="summary" aria-label="当日概览">
    <div>
      <span class="summary-label">记录时长</span>
      <strong data-testid="daily-total">
        {loading && !summary ? '—' : formatDuration(summary?.totalActiveMs ?? 0)}
      </strong>
    </div>
    <div class="tracker-state" data-state={trackerState}>
      <span class="state-dot" aria-hidden="true"></span>
      <span>{stateLabel(trackerState)}</span>
    </div>
  </section>

  {#if trackerState === 'idle' || trackerState === 'unavailable'}
    <p class="status-banner" role="status">
      {trackerState === 'idle' ? '当前空闲时间不计入使用时长。' : '当前 Windows 状态无法记录使用时长。'}
    </p>
  {:else if trackerState === 'error' && summary}
    <p class="status-banner error-banner" role="status">
      本地保存暂时异常，未提交时长会继续保留并重试。
    </p>
  {/if}

  <section class="usage-list" aria-labelledby="application-list-title">
    <div class="section-heading">
      <h2 id="application-list-title">应用</h2>
      {#if summary && applications.length > 0}
        <span>{applications.length} 个</span>
      {/if}
    </div>

    <div class="list-content">
      {#if loading && !summary}
        <div class="state-panel" role="status">
          <strong>正在读取</strong>
          <span>请稍候…</span>
        </div>
      {:else if errorMessage}
        <div class="state-panel error-state" role="alert">
          <strong>暂时无法显示</strong>
          <span>{errorMessage}</span>
          <button type="button" onclick={() => void refresh(true)}>重试</button>
        </div>
      {:else if applications.length === 0}
        <div class="state-panel empty-state">
          <strong>这一天还没有记录</strong>
          <span>总时长为 0 分钟</span>
        </div>
      {:else}
        <ul aria-label="应用使用明细">
          {#each applications as application (application.executableName + application.displayName)}
            <li data-testid="app-row">
              <div class="row-main">
                <div class="app-identity">
                  <strong title={application.displayName}>{application.displayName}</strong>
                  <span title={application.executableName}>{application.executableName}</span>
                </div>
                <div class="usage-values">
                  <strong>{formatDuration(application.activeMs)}</strong>
                  <span>{formatPercentage(application.share)}</span>
                </div>
              </div>
              <div
                class="progress-track"
                role="progressbar"
                aria-label={`${application.displayName}占比`}
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow={Math.round(Math.max(0, Math.min(1, application.share)) * 100)}
              >
                <span style={`width: ${Math.max(0, Math.min(1, application.share)) * 100}%`}></span>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </section>
</main>
