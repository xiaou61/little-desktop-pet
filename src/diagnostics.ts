import './diagnostics.css';
import Diagnostics from './Diagnostics.svelte';
import { mount } from 'svelte';

const target = document.getElementById('app');
if (!target) throw new Error('诊断中心入口缺少 app 容器。');

mount(Diagnostics, { target });
