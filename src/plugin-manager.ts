import './plugin-manager.css';
import { installGlobalDiagnosticHandlers, toFrontendDiagnosticEvent } from './lib/diagnostics';
import { recordDiagnosticEvent } from './lib/api';

installGlobalDiagnosticHandlers('plugin-manager');

const target = document.getElementById('app');

if (!target) {
  throw new Error('插件管理入口缺少 app 容器。');
}

try {
  const [{ default: PluginManager }, { mount }] = await Promise.all([
    import('./PluginManager.svelte'),
    import('svelte')
  ]);
  target.replaceChildren();
  mount(PluginManager, { target });
} catch (error) {
  void recordDiagnosticEvent(toFrontendDiagnosticEvent(error, 'plugin-manager-bootstrap')).catch(() => undefined);
  const panel = document.createElement('section');
  panel.className = 'plugin-manager-bootstrap-error';

  const heading = document.createElement('h1');
  heading.textContent = '插件管理暂时无法加载';
  panel.append(heading);

  const message = document.createElement('p');
  message.textContent = '请点击重试，或查看开发者工具中的 Console。';
  panel.append(message);

  if (import.meta.env.DEV) {
    const details = document.createElement('pre');
    details.textContent = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
    panel.append(details);
  }

  const retry = document.createElement('button');
  retry.type = 'button';
  retry.textContent = '重新加载';
  retry.addEventListener('click', () => window.location.reload());
  panel.append(retry);

  target.replaceChildren(panel);
  console.error('[plugin-manager] bootstrap failed', error);
}
