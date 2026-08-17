<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

  import {
    disablePlugin,
    enablePlugin,
    fetchPluginContributions,
    fetchPluginDirectory,
    installOfficialPlugin,
    installPluginPackage,
    previewPluginPackage,
    uninstallPlugin
  } from './lib/api';
  import type {
    PluginCatalogEntry,
    PluginContribution,
    PluginDirectory,
    PluginSource,
    PluginState,
    PluginSummary
  } from './lib/types';
  import PluginContributionRenderer from './PluginContributionRenderer.svelte';

  let directory: PluginDirectory = { installed: [], available: [] };
  let loading = true;
  let busyId: string | null = null;
  let errorMessage: string | null = null;
  let statusMessage: string | null = null;
  let preview: PluginSummary | null = null;
  let selectedPath = '';
  let contributions: Record<string, PluginContribution[]> = {};

  onMount(() => {
    void refresh();
  });

  async function refresh(): Promise<void> {
    loading = true;
    errorMessage = null;
    try {
      directory = await fetchPluginDirectory();
      const enabled = directory.installed.filter((plugin) => plugin.state === 'enabled');
      await Promise.all(
        enabled.map(async (plugin) => {
          contributions[plugin.id] = await fetchPluginContributions(plugin.id).catch(() => []);
        })
      );
      contributions = { ...contributions };
    } catch {
      errorMessage = '插件目录暂时无法读取。';
    } finally {
      loading = false;
    }
  }

  async function installOfficial(entry: PluginCatalogEntry): Promise<void> {
    busyId = entry.id;
    clearFeedback();
    try {
      await installOfficialPlugin(entry.id);
      statusMessage = `已安装${entry.displayName}，请继续启用。`;
      await refresh();
    } catch (error) {
      errorMessage = pluginErrorMessage(error, '官方插件安装失败。');
    } finally {
      busyId = null;
    }
  }

  async function toggle(plugin: PluginSummary): Promise<void> {
    busyId = plugin.id;
    clearFeedback();
    try {
      if (plugin.state === 'enabled') {
        await disablePlugin(plugin.id);
        statusMessage = `已禁用${plugin.displayName}。`;
      } else {
        await enablePlugin(plugin.id);
        statusMessage = `已启用${plugin.displayName}。`;
      }
      await refresh();
    } catch (error) {
      errorMessage = pluginErrorMessage(error, '插件状态更新失败。');
      await refresh();
    } finally {
      busyId = null;
    }
  }

  async function remove(plugin: PluginSummary): Promise<void> {
    if (plugin.protected) {
      errorMessage = '核心宿主、插件管理器和默认皮肤受保护，不能卸载。';
      return;
    }
    if (!window.confirm(`确定卸载${plugin.displayName}吗？`)) return;
    busyId = plugin.id;
    clearFeedback();
    try {
      await uninstallPlugin(plugin.id);
      statusMessage = `已卸载${plugin.displayName}。`;
      await refresh();
    } catch (error) {
      errorMessage = pluginErrorMessage(error, '插件卸载失败。');
    } finally {
      busyId = null;
    }
  }

  async function choosePackage(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    selectedPath = file ? (file as File & { path?: string }).path ?? '' : '';
    preview = null;
    clearFeedback();
    if (!selectedPath) {
      errorMessage = '当前环境无法读取本地文件路径，请从桌宠内打开本地文件选择器。';
      return;
    }
    try {
      preview = await previewPluginPackage(selectedPath);
    } catch (error) {
      errorMessage = pluginErrorMessage(error, '插件包校验失败。');
    }
  }

  async function confirmImport(): Promise<void> {
    if (!preview || !selectedPath) return;
    busyId = preview.id;
    clearFeedback();
    try {
      await installPluginPackage(selectedPath);
      statusMessage = `已导入${preview.displayName}，请确认启用。`;
      preview = null;
      selectedPath = '';
      await refresh();
    } catch (error) {
      errorMessage = pluginErrorMessage(error, '插件安装失败，现有插件未改变。');
    } finally {
      busyId = null;
    }
  }

  async function close(): Promise<void> {
    await getCurrentWebviewWindow().close().catch(() => undefined);
  }

  function clearFeedback(): void {
    errorMessage = null;
    statusMessage = null;
  }

  function stateLabel(state: PluginState): string {
    return {
      discovered: '待确认',
      installed: '已安装',
      enabled: '已启用',
      disabled: '已禁用',
      broken: '故障',
      removed: '已移除'
    }[state];
  }

  function sourceLabel(source: PluginSource): string {
    return {
      builtIn: '核心内置',
      officialDirectory: '官方目录',
      localImport: '本地导入'
    }[source];
  }

  function contributionLabel(value: string): string {
    return {
      skins: '皮肤',
      panelCards: '面板卡片',
      settings: '设置',
      menus: '菜单'
    }[value] ?? value;
  }

  function pluginErrorMessage(error: unknown, fallback: string): string {
    if (typeof error === 'object' && error !== null && 'message' in error) {
      const message = String((error as { message?: unknown }).message);
      return message.length > 160 ? fallback : message;
    }
    return fallback;
  }
</script>

<svelte:head>
  <title>插件管理</title>
</svelte:head>

<main class="plugin-manager" aria-busy={loading || busyId !== null}>
  <header class="manager-header">
    <div>
      <p class="eyebrow">小桌宠 · 本地能力</p>
      <h1>插件管理</h1>
    </div>
    <button class="icon-button" type="button" aria-label="关闭插件管理" title="关闭" onclick={() => void close()}>
      <span aria-hidden="true">×</span>
    </button>
  </header>

  <section class="import-section" aria-labelledby="import-title">
    <div class="section-heading">
      <div>
        <h2 id="import-title">导入本地插件</h2>
        <p>只接受经过校验的 .petpack 声明式资源包</p>
      </div>
      <label class="file-button">
        选择文件
        <input type="file" accept=".petpack,application/zip" onchange={choosePackage} />
      </label>
    </div>
    {#if preview}
      <div class="preview" aria-label="插件导入预览">
        <div>
          <strong>{preview.displayName}</strong>
          <span>{preview.version} · {preview.kind}</span>
        </div>
        <p>贡献：{preview.contributions.map(contributionLabel).join('、') || '无'}</p>
        <p>权限：{preview.permissions.length > 0 ? preview.permissions.join('、') : '无需额外权限'}</p>
        <button type="button" class="primary-action" disabled={busyId !== null} onclick={() => void confirmImport()}>
          确认安装
        </button>
      </div>
    {/if}
  </section>

  {#if errorMessage}
    <p class="feedback error" role="alert">{errorMessage}</p>
  {:else if statusMessage}
    <p class="feedback" role="status">{statusMessage}</p>
  {/if}

  <section class="plugin-section" aria-labelledby="installed-title">
    <div class="section-heading">
      <div>
        <h2 id="installed-title">已安装</h2>
        <p>状态保存在本地，关闭管理界面不会撤销插件</p>
      </div>
      <button type="button" class="secondary-action" disabled={loading} onclick={() => void refresh()}>刷新</button>
    </div>
    {#if loading}
      <p class="state-panel" role="status">正在读取插件状态…</p>
    {:else if directory.installed.length === 0}
      <p class="state-panel">暂无已安装插件。</p>
    {:else}
      <div class="plugin-list">
        {#each directory.installed as plugin (plugin.id)}
          <article class="plugin-card" data-state={plugin.state}>
            <div class="plugin-card-header">
              <div>
                <h3>{plugin.displayName}</h3>
                <p>{plugin.version} · {sourceLabel(plugin.source)} · {plugin.id}</p>
              </div>
              <span class="state-badge">{stateLabel(plugin.state)}</span>
            </div>
            <p class="plugin-meta">贡献：{plugin.contributions.map(contributionLabel).join('、') || '宿主能力'} · 权限：{plugin.permissions.length > 0 ? plugin.permissions.join('、') : '无'}</p>
            {#if plugin.state === 'broken' && plugin.lastError}
              <p class="plugin-error" role="alert">{plugin.lastError}</p>
            {/if}
            {#if contributions[plugin.id]?.length > 0}
              <div class="contributions" aria-label={`${plugin.displayName}贡献`}>
                {#each contributions[plugin.id] as contribution (contribution.id)}
                  <PluginContributionRenderer {contribution} />
                {/each}
              </div>
            {/if}
            <div class="plugin-actions">
              {#if plugin.state === 'enabled' || plugin.state === 'broken'}
                <button type="button" class="secondary-action" disabled={busyId === plugin.id || plugin.protected && plugin.id === 'simple-cloud'} onclick={() => void toggle(plugin)}>禁用</button>
              {:else if plugin.state === 'installed' || plugin.state === 'disabled'}
                <button type="button" class="primary-action" disabled={busyId === plugin.id} onclick={() => void toggle(plugin)}>启用</button>
              {/if}
              {#if !plugin.protected}
                <button type="button" class="danger-action" disabled={busyId === plugin.id || plugin.state === 'enabled'} onclick={() => void remove(plugin)}>卸载</button>
              {/if}
            </div>
          </article>
        {/each}
      </div>
    {/if}
  </section>

  <section class="plugin-section" aria-labelledby="catalog-title">
    <div class="section-heading">
      <div>
        <h2 id="catalog-title">官方目录</h2>
        <p>离线可用，未安装皮肤不会出现在桌宠设置中</p>
      </div>
    </div>
    <div class="catalog-list">
      {#each directory.available as entry (entry.id)}
        <article class="catalog-card" class:catalog-installed={entry.installed}>
          {#if entry.thumbnailDataUrl}
            <img src={entry.thumbnailDataUrl} alt="" />
          {/if}
          <div class="catalog-content">
            <h3>{entry.displayName}</h3>
            <p>{entry.version} · {sourceLabel(entry.source)}</p>
            <p>{entry.contributions.map(contributionLabel).join('、')} · {entry.permissions.length > 0 ? entry.permissions.join('、') : '无需额外权限'}</p>
          </div>
          {#if entry.installed}
            <span class="state-badge">已安装</span>
          {:else}
            <button type="button" class="secondary-action" disabled={busyId === entry.id} onclick={() => void installOfficial(entry)}>安装</button>
          {/if}
        </article>
      {/each}
    </div>
  </section>
</main>
