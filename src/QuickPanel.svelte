<script lang="ts">
  import { onMount, tick } from 'svelte';

  import {
    applyPetSize,
    applyPetSkin,
    closeQuickPanel,
    fetchDailyUsage,
    fetchCurrentPetSkin,
    fetchPetSize,
    fetchPetSkins,
    fetchQuickPanelEnvironment,
    markQuickPanelInternalAction,
    notifyQuickPanelReady,
    openFullStatistics,
    previewPetSize
  } from './lib/api';
  import { toLocalDateString } from './lib/date';
  import { formatDuration } from './lib/format';
  import { normalizePetSize, topApplications } from './lib/quick-panel';
  import type {
    DailyUsageSummary,
    PetSkinOption,
    QuickPanelEnvironment,
    TrackerState
  } from './lib/types';

  let summary: DailyUsageSummary | null = null;
  let summaryLoading = true;
  let summaryError = false;
  let view: 'summary' | 'settings' = 'summary';
  let sizePercent = 100;
  let draftSize = 100;
  let sizeLoading = true;
  let applyingSize = false;
  let previewingSize = false;
  let pendingSize: number | null = null;
  let pendingCommit: number | null = null;
  let sizeMessage: string | null = null;
  let sizeError: string | null = null;
  let skins: PetSkinOption[] = [];
  let currentSkinId = 'simple-cloud';
  let skinsLoading = true;
  let applyingSkin = false;
  let skinMessage: string | null = null;
  let skinError: string | null = null;
  let closing = false;
  let settingsHeading: HTMLHeadingElement | null = null;
  let settingsButton: HTMLButtonElement | null = null;
  let environment: QuickPanelEnvironment = {
    glassAvailable: false,
    highContrast: false,
    reduceMotion: false,
    lastError: null
  };

  $: applications = topApplications(summary?.applications ?? []);
  $: trackerState = summary?.trackerState ?? 'unavailable';

  onMount(() => {
    void loadSummary();
    void loadSettings();
    void loadSkins();
    void loadEnvironment();
    void tick().then(() => notifyQuickPanelReady()).catch(() => undefined);
  });

  async function loadSummary(): Promise<void> {
    summaryLoading = true;
    summaryError = false;
    try {
      summary = await fetchDailyUsage(toLocalDateString());
    } catch {
      summary = null;
      summaryError = true;
    } finally {
      summaryLoading = false;
    }
  }

  async function loadSettings(): Promise<void> {
    sizeLoading = true;
    try {
      const status = await fetchPetSize();
      sizePercent = normalizePetSize(status.sizePercent);
      draftSize = sizePercent;
    } catch {
      sizePercent = 100;
      draftSize = 100;
      sizeError = '当前大小暂时无法读取，已显示默认值。';
    } finally {
      sizeLoading = false;
    }
  }

  async function loadEnvironment(): Promise<void> {
    try {
      environment = await fetchQuickPanelEnvironment();
    } catch {
      environment = { ...environment, glassAvailable: false };
    }
  }

  async function loadSkins(): Promise<void> {
    skinsLoading = true;
    skinError = null;
    try {
      const [available, status] = await Promise.all([fetchPetSkins(), fetchCurrentPetSkin()]);
      skins = available;
      currentSkinId = available.some((skin) => skin.id === status.skinId)
        ? status.skinId
        : 'simple-cloud';
    } catch {
      skins = [];
      currentSkinId = 'simple-cloud';
      skinError = '皮肤清单暂时无法读取。';
    } finally {
      skinsLoading = false;
    }
  }

  async function showSettings(): Promise<void> {
    view = 'settings';
    sizeMessage = null;
    await tick();
    settingsHeading?.focus();
  }

  async function showSummary(): Promise<void> {
    view = 'summary';
    await tick();
    settingsButton?.focus();
  }

  function queueSizePreview(value: number): void {
    const normalized = normalizePetSize(value);
    draftSize = normalized;
    pendingSize = normalized;
    sizeMessage = null;
    sizeError = null;
    if (!previewingSize && !applyingSize) {
      void applyPendingPreview();
    }
  }

  async function applyPendingPreview(): Promise<void> {
    if (previewingSize || applyingSize || pendingSize === null) return;
    const requestedSize = pendingSize;
    pendingSize = null;
    previewingSize = true;
    try {
      const result = await previewPetSize(requestedSize);
      sizePercent = normalizePetSize(result.sizePercent);
      if (pendingSize === null && pendingCommit === null) {
        draftSize = sizePercent;
      }
    } catch {
      if (pendingSize === null && pendingCommit === null) {
        draftSize = sizePercent;
        sizeError = '大小预览失败，已保留原来的设置。';
      }
    } finally {
      previewingSize = false;
      if (pendingCommit !== null) {
        void applyPendingCommit();
      } else if (pendingSize !== null) {
        void applyPendingPreview();
      }
    }
  }

  function commitSize(value: number): void {
    const normalized = normalizePetSize(value);
    draftSize = normalized;
    pendingSize = null;
    pendingCommit = normalized;
    if (!previewingSize && !applyingSize) {
      void applyPendingCommit();
    }
  }

  async function selectSkin(skin: PetSkinOption): Promise<void> {
    if (!skin.available || applyingSkin || skin.id === currentSkinId) return;
    const previousSkinId = currentSkinId;
    applyingSkin = true;
    skinMessage = null;
    skinError = null;
    try {
      const result = await applyPetSkin(skin.id);
      currentSkinId = result.skinId;
      if (result.saved) {
        skinMessage = `已应用${skin.displayName}`;
      } else {
        skinError = result.message ?? '皮肤已应用，但本地保存失败。';
      }
    } catch (error) {
      currentSkinId = previousSkinId;
      skinError = skinErrorMessage(error);
    } finally {
      applyingSkin = false;
    }
  }

  function skinErrorMessage(error: unknown): string {
    const code =
      typeof error === 'object' && error !== null && 'code' in error
        ? String((error as { code?: unknown }).code)
        : '';
    switch (code) {
      case 'pet_skin_resource_failed':
        return '皮肤资源加载失败，已保留原来的外观。';
      case 'pet_skin_frame_failed':
        return '皮肤切换失败，已保留原来的外观。';
      case 'pet_skin_unknown_id':
        return '该皮肤暂不可用。';
      default:
        return '皮肤切换失败，已保留原来的外观。';
    }
  }

  async function applyPendingCommit(): Promise<void> {
    if (previewingSize || applyingSize || pendingCommit === null) return;
    const requestedSize = pendingCommit;
    pendingCommit = null;
    applyingSize = true;
    sizeMessage = null;
    sizeError = null;
    try {
      const result = await applyPetSize(requestedSize);
      sizePercent = normalizePetSize(result.sizePercent);
      if (pendingCommit === null && pendingSize === null) {
        draftSize = sizePercent;
        if (result.saved) {
          sizeMessage = `已应用并保存 ${sizePercent}%`;
        } else {
          sizeError = result.message ?? '大小已应用，但本地保存失败。';
        }
      }
    } catch {
      if (pendingCommit === null && pendingSize === null) {
        draftSize = sizePercent;
        sizeError = '大小调整失败，已保留原来的设置。';
      }
    } finally {
      applyingSize = false;
      if (pendingCommit !== null) {
        void applyPendingCommit();
      } else if (pendingSize !== null) {
        void applyPendingPreview();
      }
    }
  }

  async function requestClose(): Promise<void> {
    if (closing) return;
    closing = true;
    if (!environment.reduceMotion) {
      await new Promise((resolve) => window.setTimeout(resolve, 90));
    }
    await closeQuickPanel().catch(() => undefined);
  }

  async function openStatistics(): Promise<void> {
    await openFullStatistics().catch(() => {
      summaryError = true;
    });
  }

  function handleKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      event.preventDefault();
      void requestClose();
    }
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
  <title>小桌宠快捷面板</title>
</svelte:head>

<svelte:window onkeydown={handleKeydown} />

<main
  class="quick-panel"
  class:glass={environment.glassAvailable}
  class:fallback={!environment.glassAvailable}
  class:high-contrast={environment.highContrast}
  class:reduce-motion={environment.reduceMotion}
  class:closing
  aria-busy={summaryLoading || sizeLoading || applyingSize || skinsLoading || applyingSkin}
  onpointerdown={() => void markQuickPanelInternalAction().catch(() => undefined)}
>
  {#if view === 'summary'}
    <header class="panel-header">
      <div class="panel-heading">
        <p>小桌宠</p>
        <h1>今天</h1>
      </div>
      <button
        class="icon-button panel-close"
        type="button"
        aria-label="关闭快捷面板"
        title="关闭"
        onclick={() => void requestClose()}
      >
        <span aria-hidden="true">×</span>
      </button>
    </header>

    <section class="quick-summary" aria-label="今日记录概览">
      <div>
        <span>活跃时长</span>
        <strong data-testid="quick-total">
          {summaryLoading && !summary ? '—' : formatDuration(summary?.totalActiveMs ?? 0)}
        </strong>
      </div>
      <div class="quick-state" data-state={trackerState}>
        <span class="state-dot" aria-hidden="true"></span>
        <span>{stateLabel(trackerState)}</span>
      </div>
    </section>

    <section class="quick-usage" aria-labelledby="quick-usage-title">
      <h2 id="quick-usage-title">使用概览</h2>
      {#if summaryLoading && !summary}
        <p class="panel-state" role="status">正在读取今日记录…</p>
      {:else if summaryError}
        <div class="panel-state panel-error" role="alert">
          <span>今日概览暂时无法读取。</span>
          <button type="button" onclick={() => void loadSummary()}>重试</button>
        </div>
      {:else if applications.length === 0}
        <p class="panel-state panel-empty">今天还没有使用记录</p>
      {:else}
        <ul aria-label="今日使用最多的应用">
          {#each applications as application (application.executableName + application.displayName)}
            <li data-testid="quick-app-row">
              <span title={application.displayName}>{application.displayName}</span>
              <strong>{formatDuration(application.activeMs)}</strong>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <footer class="panel-actions">
      <button class="primary-action" type="button" onclick={() => void openStatistics()}>
        查看完整统计
      </button>
      <button
        class="secondary-action"
        type="button"
        bind:this={settingsButton}
        onclick={() => void showSettings()}
      >
        设置
      </button>
    </footer>
  {:else}
    <header class="panel-header settings-header">
      <button
        class="icon-button"
        type="button"
        aria-label="返回今日概览"
        title="返回"
        onclick={() => void showSummary()}
      >
        <span aria-hidden="true">‹</span>
      </button>
      <h1 bind:this={settingsHeading} tabindex="-1">桌宠设置</h1>
      <button
        class="icon-button panel-close"
        type="button"
        aria-label="关闭快捷面板"
        title="关闭"
        onclick={() => void requestClose()}
      >
        <span aria-hidden="true">×</span>
      </button>
    </header>

    <section class="size-settings" aria-labelledby="pet-size-title">
      <div class="setting-title">
        <div>
          <h2 id="pet-size-title">桌宠大小</h2>
          <p>调整后立即应用</p>
        </div>
        <output for="pet-size">{draftSize}%</output>
      </div>
      <input
        id="pet-size"
        type="range"
        min="30"
        max="160"
        step="10"
        bind:value={draftSize}
        aria-label="桌宠大小"
        aria-valuetext={`${draftSize}%`}
        disabled={sizeLoading}
        oninput={() => queueSizePreview(draftSize)}
        onchange={() => commitSize(draftSize)}
      />
      <div class="range-labels" aria-hidden="true">
        <span>30%</span>
        <span>100%</span>
        <span>160%</span>
      </div>
      <button
        class="default-action"
        type="button"
        disabled={sizeLoading || draftSize === 100}
        onclick={() => commitSize(100)}
      >
        恢复默认大小
      </button>

      <section class="skin-settings" aria-labelledby="pet-skin-title">
        <div class="setting-title">
          <div>
            <h2 id="pet-skin-title">桌宠外观</h2>
            <p>选择后立即应用</p>
          </div>
        </div>
        {#if skinsLoading}
          <div class="skin-gallery skin-gallery-loading" aria-busy="true" role="status">
            <span class="skin-placeholder" aria-hidden="true"></span>
            <span class="skin-placeholder" aria-hidden="true"></span>
            <span class="skin-placeholder" aria-hidden="true"></span>
          </div>
        {:else if skins.length === 0}
          <p class="skin-empty" role="status">暂无可用皮肤</p>
        {:else}
          <div class="skin-gallery" aria-label="内置皮肤">
            {#each skins as skin (skin.id)}
              <button
                class="skin-option"
                class:skin-selected={skin.id === currentSkinId}
                type="button"
                aria-pressed={skin.id === currentSkinId}
                aria-label={`选择${skin.displayName}`}
                disabled={applyingSkin || !skin.available}
                onclick={() => void selectSkin(skin)}
              >
                <span class="skin-preview" aria-hidden="true">
                  <img src={skin.thumbnailDataUrl} alt="" />
                </span>
                <span class="skin-name">{skin.displayName}</span>
                {#if skin.id === currentSkinId}
                  <span class="skin-current">当前</span>
                {:else if !skin.available}
                  <span class="skin-unavailable">不可用</span>
                {/if}
              </button>
            {/each}
          </div>
        {/if}
        <div class="skin-feedback" aria-live="polite">
          {#if applyingSkin}
            <p role="status">正在应用外观…</p>
          {:else if skinError}
            <p class="setting-error" role="alert">{skinError}</p>
          {:else if skinMessage}
            <p role="status">{skinMessage}</p>
          {/if}
        </div>
      </section>

      <div class="setting-feedback" aria-live="polite">
        {#if applyingSize}
          <p role="status">正在应用…</p>
        {:else if sizeError}
          <p class="setting-error" role="alert">{sizeError}</p>
        {:else if sizeMessage}
          <p role="status">{sizeMessage}</p>
        {:else if environment.lastError}
          <p class="setting-error" role="alert">{environment.lastError}</p>
        {/if}
      </div>
    </section>
  {/if}
</main>
