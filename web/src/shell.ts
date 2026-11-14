import { mount } from 'svelte';
import Shell from './Shell.svelte';
import './app.css';

mount(Shell, { target: document.getElementById('shell')! });
