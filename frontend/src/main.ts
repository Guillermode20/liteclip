import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'

const target = document.getElementById('app');
if (!target) {
  throw new Error('App mount target #app not found');
}

console.log('Mounting Svelte app...');
mount(App, { target });

function sendNativeMessage(message: string): boolean {
  try {
    if (typeof window === 'undefined') {
      return false;
    }

    const host = window as typeof window & {
      Photino?: { sendWebMessage?: (message: string) => void }
      external?: { sendMessage?: (message: string) => void }
    }

    if (host.Photino?.sendWebMessage) {
      host.Photino.sendWebMessage(message);
      return true;
    }

    if (host.external?.sendMessage) {
      host.external.sendMessage(message);
      return true;
    }
  } catch (error) {
    console.error('Sending native message failed:', error);
  }

  return false;
}

// Send message to C# that the app is ready with retry logic
function sendWindowReady(attempt = 0) {
  if (sendNativeMessage('window-ready')) {
    console.log('Sent window-ready message to host');
    return;
  }

  const nextDelay = Math.min(500, 50 + attempt * 25);
  console.log(`Native host not ready yet (attempt ${attempt + 1}). Retrying in ${nextDelay}ms...`);
  setTimeout(() => sendWindowReady(attempt + 1), nextDelay);
}

sendWindowReady();
