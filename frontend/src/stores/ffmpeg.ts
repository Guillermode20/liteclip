import { writable, derived } from 'svelte/store';
import type { FfmpegStatusResponse } from '../types';
import { getFfmpegStatus, retryFfmpeg as apiRetryFfmpeg } from '../services/api';

interface FfmpegStoreState {
    ready: boolean;
    state: FfmpegStatusResponse['state'];
    message: string;
    progressPercent: number;
    error: string | null;
    retrying: boolean;
}

const initialState: FfmpegStoreState = {
    ready: false,
    state: 'idle',
    message: 'Preparing FFmpeg dependencies...',
    progressPercent: 0,
    error: null,
    retrying: false
};

function createFfmpegStore() {
    const { subscribe, set, update } = writable<FfmpegStoreState>(initialState);
    let pollInterval: number | null = null;

    function stopPolling() {
        if (pollInterval !== null) {
            clearInterval(pollInterval);
            pollInterval = null;
        }
    }

    async function checkStatus() {
        try {
            const status = await getFfmpegStatus();
            update(s => ({
                ...s,
                ready: status.ready,
                state: status.state,
                message: status.message ?? 'Preparing FFmpeg dependencies...',
                progressPercent: typeof status.progressPercent === 'number' ? status.progressPercent : 0,
                error: status.errorMessage ?? null
            }));

            if (status.ready) {
                stopPolling();
            }
        } catch (error) {
            update(s => ({
                ...s,
                error: (error as Error).message || 'Unable to check FFmpeg status',
                message: 'Unable to check FFmpeg status'
            }));
        }
    }

    return {
        subscribe,
        startPolling: () => {
            if (pollInterval !== null) return;
            checkStatus();
            pollInterval = window.setInterval(checkStatus, 1500);
        },
        stopPolling,
        retry: async () => {
            update(s => ({ ...s, retrying: true, message: 'Retrying FFmpeg download...', error: null }));
            try {
                await apiRetryFfmpeg();
                stopPolling();
                // Restart polling
                checkStatus();
                pollInterval = window.setInterval(checkStatus, 1500);
            } catch (error) {
                update(s => ({ ...s, error: (error as Error).message }));
            } finally {
                update(s => ({ ...s, retrying: false }));
            }
        }
    };
}

export const ffmpegStore = createFfmpegStore();
