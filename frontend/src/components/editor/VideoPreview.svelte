<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import VideoControls from './VideoControls.svelte';
    import CropOverlay from './CropOverlay.svelte';

    export let videoElement: HTMLVideoElement | null = null;
    export let videoAspectRatio: number = 16 / 9;
    export let isPlaying: boolean = false;
    export let isReady: boolean = false;
    export let currentTime: number = 0;
    export let duration: number = 0;
    export let isDragging: boolean = false;
    export let visualScrubberPosition: number = 0;

    // Crop props
    export let crop = { x: 0, y: 0, width: 100, height: 100 };
    export let isCropActive = false;

    const dispatch = createEventDispatcher<{
        loadedmetadata: void;
        timeupdate: void;
        play: void;
        pause: void;
        playPause: void;
        cut: void;
        reset: void;
        cropChange: { x: number; y: number; width: number; height: number };
    }>();
</script>

<div class="video-preview">
    <div class="video-frame" style={`--video-aspect: ${videoAspectRatio}`}>
        <!-- svelte-ignore a11y-media-has-caption -->
        <video
            bind:this={videoElement}
            on:loadedmetadata={() => dispatch('loadedmetadata')}
            on:timeupdate={() => dispatch('timeupdate')}
            on:play={() => dispatch('play')}
            on:pause={() => dispatch('pause')}
            class="editor-video"
        ></video>

        <CropOverlay
            {crop}
            isActive={isCropActive}
            on:change={(e) => dispatch('cropChange', e.detail)}
        />
    </div>
    
    <VideoControls
        {isPlaying}
        {isReady}
        {currentTime}
        {duration}
        {isDragging}
        {visualScrubberPosition}
        on:playPause
        on:cut
        on:reset
    />
</div>

<style>
    .video-preview {
        position: relative;
        background: #09090b;
        border: 1px solid #27272a;
        border-radius: 4px;
        margin-bottom: 1.5rem;
        overflow: hidden;
        display: flex;
        flex-direction: column;
    }

    .video-frame {
        position: relative;
        width: 100%;
        aspect-ratio: var(--video-aspect, 16 / 9);
        overflow: hidden;
        background: #09090b;
        border-bottom: 1px solid #27272a;
        max-width: 1280px;
        max-height: 720px;
        margin-left: auto;
        margin-right: auto;
    }

    .editor-video {
        width: 100%;
        height: 100%;
        display: block;
        object-fit: contain;
        background: #09090b;
    }
</style>
