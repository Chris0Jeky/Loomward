import { mount } from 'svelte';
import './styles/tokens.css';
import './styles/app.css';
import App from './App.svelte';
import { session } from './lib/stores/session.svelte';

mount(App, { target: document.getElementById('app')! });
void session.start();
