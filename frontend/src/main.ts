import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'

function mountApp() {
  const target = document.getElementById('app');
  if (!target) {
    console.error('App mount target #app not found, retrying...');
    setTimeout(mountApp, 50);
    return;
  }
  
  console.log('Mounting Svelte app...');
  const app = mount(App, { target });
  return app;
}

// Wait for DOM to be ready before mounting
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', mountApp);
} else {
  mountApp();
}

export default mountApp();
