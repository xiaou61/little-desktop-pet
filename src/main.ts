import './app.css';
import App from './App.svelte';
import { mount } from 'svelte';
import { installGlobalDiagnosticHandlers } from './lib/diagnostics';

installGlobalDiagnosticHandlers('dashboard');

mount(App, {
  target: document.getElementById('app')!
});
