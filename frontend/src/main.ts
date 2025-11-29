import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'

const target = document.getElementById('app');
if (!target) {
  throw new Error('App mount target #app not found');
}

console.log('Mounting Svelte app...');
mount(App, { target });

// Send message to C# that the app is ready with retry logic
function sendWindowReady() {
  try {
    if (typeof window !== 'undefined' && (window as any).Photino?.sendWebMessage) {
      console.log('Sending window-ready message to C#');
      (window as any).Photino.sendWebMessage('window-ready');
    } else {
      console.log('Photino not ready yet, retrying...');
      setTimeout(sendWindowReady, 50);
    }
  } catch (error) {
    console.error('Failed to send window-ready message:', error);
    setTimeout(sendWindowReady, 100);
  }
}

sendWindowReady();
