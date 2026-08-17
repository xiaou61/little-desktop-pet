import './quick-panel.css';
import QuickPanel from './QuickPanel.svelte';
import { mount } from 'svelte';

const target = document.getElementById('app')!;
target.replaceChildren();
mount(QuickPanel, { target });
