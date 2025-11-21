<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import type { OutputMetadata } from '../types';

    const dispatch = createEventDispatcher<{
        metadata: { duration: number | null; width: number; height: number };
        download: void;
        clear: void;
    }>();

    export let videoUrl: string | null = null;
    export let downloadMimeType = 'video/mp4';
    export let outputMetadata: OutputMetadata;
    export let originalSizeMb: number | null = null;
    export let finalBitrateLabel = '--';
    export let finalResolutionLabel = '--';
    export let finalDurationLabel = '--';
    export let encodingTimeLabel = '--';
    export let downloadDisabled = false;

    let videoElement: HTMLVideoElement | null = null;
    $: originalSizeLabel = typeof originalSizeMb === 'number' ? `${originalSizeMb.toFixed(1)} MB` : '--';

    function handleLoadedMetadata() {
        if (!videoElement) return;
        dispatch('metadata', {
            duration: isFinite(videoElement.duration) ? videoElement.duration : null,
            width: videoElement.videoWidth || 0,
            height: videoElement.videoHeight || 0
        });
    }

    function handleDownloadClick() {
        dispatch('download');
    }

    function handleClearClick() {
        dispatch('clear');
    }
</script>

<div class="content-card">
    <h2 class="section-title">// compressed_output</h2>
    <div class="video-container">
        <video
            bind:this={videoElement}
            controls
            preload="none"
            aria-label="Compressed video preview"
            tabindex="0"
            on:loadedmetadata={handleLoadedMetadata}
        >
            {#if videoUrl}
                <source src={videoUrl} type={downloadMimeType}>
            {/if}
            <track kind="captions" srclang="en" label="English" default>
            Your browser does not support the video tag.
        </video>
    </div>

    <div class="metadata-section">
        <h3 class="metadata-title">// compression_stats</h3>
        <div class="metadata-grid">
            <div class="metadata-item">
                <span class="metadata-label">output_size</span>
                <span class="metadata-value">{outputMetadata.outputSizeMb.toFixed(1)} MB</span>
            </div>
            <div class="metadata-item">
                <span class="metadata-label">compression</span>
                <span class="metadata-value" class:positive={outputMetadata.compressionRatio > 0}>
                    {outputMetadata.compressionRatio.toFixed(1)}%
                </span>
            </div>
            <div class="metadata-item">
                <span class="metadata-label">codec</span>
                <span class="metadata-value">
                    {outputMetadata.codec.toUpperCase()}
                    {#if outputMetadata.encoderName}
                        &nbsp;—&nbsp;{outputMetadata.encoderName}{outputMetadata.encoderIsHardware ? ' (hardware)' : ' (software)'}
                    {/if}
                </span>
            </div>
            <div class="metadata-item">
                <span class="metadata-label">bitrate</span>
                <span class="metadata-value">{finalBitrateLabel}</span>
            </div>
            <div class="metadata-item">
                <span class="metadata-label">resolution</span>
                <span class="metadata-value">{finalResolutionLabel}</span>
            </div>
            <div class="metadata-item">
                <span class="metadata-label">duration</span>
                <span class="metadata-value">{finalDurationLabel}</span>
            </div>
            <div class="metadata-item">
                <span class="metadata-label">original_size</span>
                <span class="metadata-value">{originalSizeLabel}</span>
            </div>
            <div class="metadata-item">
                <span class="metadata-label">encoding_time</span>
                <span class="metadata-value">{encodingTimeLabel}</span>
            </div>
        </div>
    </div>

    <div class="action-buttons">
        <button 
            id="downloadBtn" 
            on:click={handleDownloadClick} 
            class="action-btn primary"
            disabled={downloadDisabled}
        >
            $ download_compressed_video
        </button>
        <button 
            id="clearBtn"
            on:click={handleClearClick}
            class="action-btn secondary"
        >
            $ clear_and_compress_again
        </button>
    </div>
</div>

