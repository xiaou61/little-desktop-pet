<script lang="ts">
  import type { PluginContribution, PluginContributionType } from './lib/types';

  export let contribution: PluginContribution;

  const typeLabels: Record<PluginContributionType, string> = {
    skins: '皮肤',
    panelCards: '面板卡片',
    settings: '设置',
    menus: '菜单'
  };
</script>

<article class="contribution-item">
  <div>
    <strong>{contribution.label ?? contribution.id}</strong>
    <span>{typeLabels[contribution.type]}</span>
  </div>
  {#if contribution.actions.length > 0}
    <ul aria-label="固定动作">
      {#each contribution.actions as action}
        <li>{action.label ?? action.id}</li>
      {/each}
    </ul>
  {/if}
</article>
