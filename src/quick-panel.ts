import './quick-panel.css';
import QuickPanel from './QuickPanel.svelte';
import { mount } from 'svelte';
import { installGlobalDiagnosticHandlers } from './lib/diagnostics';

installGlobalDiagnosticHandlers('quick-panel');

const target = document.getElementById('app')!;
target.replaceChildren();
mount(QuickPanel, { target });
