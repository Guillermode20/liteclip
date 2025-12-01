<script lang="ts">
    import { onMount, onDestroy } from 'svelte';
    import { getFfmpegStatus, startFfmpeg, retryFfmpeg, closeApp } from '../services/api';
    import type { FfmpegStatusResponse } from '../types';

    let ffmpegReady = false;
    let ffmpegStatusChecked = false;
    let ffmpegConsentGiven = false;
    let ffmpegStatusMessage = 'Preparing FFmpeg dependencies...';
    let ffmpegProgressPercent = 0;
    let ffmpegState: FfmpegStatusResponse['state'] = 'idle';
    let ffmpegError: string | null = null;
    let ffmpegRetrying = false;
    let ffmpegStatusInterval: number | null = null;

    onMount(() => {
        checkInitialFfmpegStatus();
    });

    onDestroy(() => {
        stopFfmpegPolling();
    });

    async function checkInitialFfmpegStatus() {
        try {
            const payload = await getFfmpegStatus();
            updateStatus(payload);

            // If FFmpeg is already ready, mark consent as given and skip the modal entirely
            if (payload.ready) {
                ffmpegConsentGiven = true;
                console.log('FFmpeg already available, skipping download modal');
            }
        } catch (error) {
            console.warn('Initial FFmpeg status check failed:', error);
            ffmpegError = (error as Error).message || 'Unable to check FFmpeg status';
            ffmpegStatusMessage = 'Unable to check FFmpeg status';
        } finally {
            // Mark status as checked so UI can render appropriately
            ffmpegStatusChecked = true;
        }
    }

    function updateStatus(payload: FfmpegStatusResponse) {
        ffmpegState = payload.state;
        ffmpegReady = payload.ready;
        ffmpegProgressPercent = typeof payload.progressPercent === 'number' ? payload.progressPercent : 0;
        ffmpegStatusMessage = payload.message ?? 'Preparing FFmpeg dependencies...';
        ffmpegError = payload.errorMessage ?? null;

        if (payload.ready) {
            stopFfmpegPolling();
        }
    }

    async function fetchFfmpegStatus() {
        try {
            const payload = await getFfmpegStatus();
            updateStatus(payload);
        } catch (error) {
            ffmpegError = (error as Error).message || 'Unable to check FFmpeg status';
            ffmpegStatusMessage = 'Unable to check FFmpeg status';
        }
    }

    function startFfmpegPolling() {
        if (ffmpegStatusInterval !== null) {
            return;
        }
        fetchFfmpegStatus();
        ffmpegStatusInterval = window.setInterval(fetchFfmpegStatus, 1500);
    }

    function stopFfmpegPolling() {
        if (ffmpegStatusInterval !== null) {
            clearInterval(ffmpegStatusInterval);
            ffmpegStatusInterval = null;
        }
    }

    async function handleFfmpegConsent(accepted: boolean) {
        if (!accepted) {
            try {
                console.log('Attempting to close app...');
                
                // Try native Photino message first
                if ((window as any).external && (window as any).external.sendMessage) {
                    console.log('Using window.external.sendMessage');
                    (window as any).external.sendMessage('close-app');
                } 
                // Fallback for older Photino or different setups
                else if (typeof window !== 'undefined' && (window as any).Photino) {
                    console.log('Using window.Photino.sendWebMessage');
                    (window as any).Photino.sendWebMessage('close-app');
                }
                // Ultimate fallback: call backend endpoint to kill the process
                else {
                    console.warn('Native interop not found, calling backend kill switch...');
                    await closeApp();
                }
            } catch (e) {
                console.error('Native close failed, calling backend kill switch:', e);
                await closeApp();
            }
            return;
        }

        ffmpegConsentGiven = true;
        try {
            await startFfmpeg();
        } catch (e) {
            ffmpegError = (e as Error).message;
            ffmpegStatusMessage = 'Failed to start FFmpeg download';
            return;
        }
        startFfmpegPolling();
    }

    async function handleFfmpegRetry() {
        if (ffmpegRetrying) return;
        ffmpegRetrying = true;
        ffmpegStatusMessage = 'Retrying FFmpeg download...';
        try {
            await retryFfmpeg();
            ffmpegError = null;
            stopFfmpegPolling();
            startFfmpegPolling();
        } catch (error) {
            ffmpegError = (error as Error).message;
        } finally {
            ffmpegRetrying = false;
        }
    }
</script>

{#if ffmpegStatusChecked && !ffmpegReady}
    <div class="ffmpeg-overlay">
        <div class="ffmpeg-card">
            {#if !ffmpegConsentGiven}
                <h2>Download FFmpeg to get started</h2>
                <p class="ffmpeg-message">
                    LiteClip uses FFmpeg, a small open-source video tool, to compress and trim your videos.
                </p>
                <p class="ffmpeg-message">
                    The app will download the FFmpeg program file it needs and store it locally. It does not install
                    anything else or change your PC.
                </p>
                <div class="action-buttons">
                    <button class="action-btn primary" on:click={() => handleFfmpegConsent(true)}>
                        download ffmpeg and continue
                    </button>
                    <button class="action-btn secondary" on:click={() => handleFfmpegConsent(false)}>
                        close app
                    </button>
                </div>
            {:else}
                <h2>Preparing FFmpeg…</h2>
                <p class="ffmpeg-message">{ffmpegStatusMessage}</p>
                <div class="ffmpeg-progress">
                    <div class="ffmpeg-progress-track">
                        <div
                            class="ffmpeg-progress-fill"
                            style={`width: ${Math.min(100, Math.max(5, ffmpegProgressPercent || 0)).toFixed(1)}%;`}
                        ></div>
                    </div>
                    <span>{Math.max(0, Math.min(100, ffmpegProgressPercent || 0)).toFixed(1)}%</span>
                </div>
                <span class="ffmpeg-state">{ffmpegState.toUpperCase()}</span>
                {#if ffmpegError}
                    <p class="ffmpeg-error">{ffmpegError}</p>
                    <button class="retry-btn" on:click={handleFfmpegRetry} disabled={ffmpegRetrying}>
                        {ffmpegRetrying ? 'retrying...' : 'retry download'}
                    </button>
                {/if}
            {/if}
        </div>
    </div>
{/if}

<style>
    .ffmpeg-overlay {
        position: fixed;
        top: 0;
        left: 0;
        right: 0;
        bottom: 0;
        background: rgba(0, 0, 0, 0.85);
        backdrop-filter: blur(8px);
        z-index: 9999;
        display: flex;
        align-items: center;
        justify-content: center;
        animation: fadeIn 0.3s ease-out;
    }

    .ffmpeg-card {
        background: var(--bg-card);
        border: 1px solid var(--border-color);
        border-radius: 16px;
        padding: 2.5rem;
        width: 100%;
        max-width: 500px;
        text-align: center;
        box-shadow: 0 20px 40px rgba(0, 0, 0, 0.4);
    }

    .ffmpeg-card h2 {
        margin: 0 0 1rem;
        font-size: 1.5rem;
        font-weight: 600;
        color: var(--text-primary);
    }

    .ffmpeg-message {
        color: var(--text-secondary);
        margin-bottom: 1.5rem;
        line-height: 1.6;
    }

    .ffmpeg-progress {
        display: flex;
        align-items: center;
        gap: 1rem;
        margin: 1.5rem 0;
    }

    .ffmpeg-progress-track {
        flex: 1;
        height: 8px;
        background: var(--bg-secondary);
        border-radius: 4px;
        overflow: hidden;
    }

    .ffmpeg-progress-fill {
        height: 100%;
        background: var(--primary-color);
        transition: width 0.3s ease;
    }

    .ffmpeg-state {
        display: inline-block;
        padding: 0.25rem 0.75rem;
        background: var(--bg-secondary);
        border-radius: 4px;
        font-size: 0.75rem;
        font-weight: 600;
        color: var(--text-secondary);
        margin-bottom: 1rem;
    }

    .ffmpeg-error {
        color: #ef4444;
        background: rgba(239, 68, 68, 0.1);
        padding: 0.75rem;
        border-radius: 8px;
        margin-bottom: 1rem;
        font-size: 0.9rem;
    }

    .action-buttons {
        display: flex;
        gap: 1rem;
        justify-content: center;
        margin-top: 2rem;
    }

    .action-btn {
        padding: 0.75rem 1.5rem;
        border-radius: 8px;
        font-weight: 500;
        cursor: pointer;
        transition: all 0.2s;
        border: none;
        font-size: 0.95rem;
    }

    .action-btn.primary {
        background: var(--primary-color);
        color: white;
    }

    .action-btn.primary:hover {
        background: var(--primary-hover);
        transform: translateY(-1px);
    }

    .action-btn.secondary {
        background: transparent;
        border: 1px solid var(--border-color);
        color: var(--text-secondary);
    }

    .action-btn.secondary:hover {
        background: var(--bg-secondary);
        color: var(--text-primary);
    }

    .retry-btn {
        background: transparent;
        border: 1px solid var(--primary-color);
        color: var(--primary-color);
        padding: 0.5rem 1rem;
        border-radius: 6px;
        cursor: pointer;
        font-size: 0.9rem;
        transition: all 0.2s;
    }

    .retry-btn:hover:not(:disabled) {
        background: rgba(var(--primary-rgb), 0.1);
    }

    .retry-btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }

    @keyframes fadeIn {
        from { opacity: 0; }
        to { opacity: 1; }
    }
</style>
