<script lang="ts">
    import { onMount } from 'svelte';
    import type { VideoSegment } from './lib/types';

    export let videoFile: File;
    export let onSegmentsChange: (segments: Array<{start: number, end: number}>) => void;
    export let onRemoveVideo: (() => void) | null = null;
    export let savedSegments: VideoSegment[] = [];

    let videoElement: HTMLVideoElement;
    let canvasElement: HTMLCanvasElement;
    let timelineContainer: HTMLDivElement;
    
    let duration: number = 0;
    let currentTime: number = 0;
    let isPlaying: boolean = false;
    let isReady: boolean = false;
    let isDragging: boolean = false;
    let wasPlayingBeforeDrag: boolean = false;
    let videoAspectRatio = 16 / 9;
    
    // Trim segments (kept segments)
    let segments: Array<{start: number, end: number, id: string}> = [];
    let nextSegmentId = 1;
    
    function clampTime(value: number) {
        if (!Number.isFinite(value)) return 0;
        if (value < 0) return 0;
        if (value > duration) return duration;
        return value;
    }

    function createSegmentRange(start: number, end: number) {
        return {
            start,
            end,
            id: `seg-${nextSegmentId++}`
        };
    }

    function sanitizeSavedSegments(): Array<{ start: number; end: number }> {
        if (!savedSegments || savedSegments.length === 0 || !Number.isFinite(duration) || duration <= 0) {
            return [];
        }

        return savedSegments
            .map(seg => ({
                start: clampTime(seg.start),
                end: clampTime(seg.end)
            }))
            .filter(seg => seg.end > seg.start)
            .sort((a, b) => a.start - b.start);
    }

    function initializeSegmentsFromSavedState() {
        const restored = sanitizeSavedSegments();
        if (restored.length > 0) {
            segments = restored.map(seg => createSegmentRange(seg.start, seg.end));
            return;
        }

        segments = [createSegmentRange(0, duration)];
    }
    
    // No selection or dragging - we just cut and delete

    onMount(() => {
        if (videoFile) {
            loadVideo();
        }
        window.addEventListener('keydown', handleKeyDown);
        window.addEventListener('mouseup', handleMouseUp);
        window.addEventListener('mousemove', handleMouseMove);
        return () => {
            window.removeEventListener('keydown', handleKeyDown);
            window.removeEventListener('mouseup', handleMouseUp);
            window.removeEventListener('mousemove', handleMouseMove);
        };
    });

    function loadVideo() {
        const url = URL.createObjectURL(videoFile);
        videoElement.src = url;
        videoElement.load();
    }

    function handleLoadedMetadata() {
        const loadedDuration = videoElement.duration;
        duration = Number.isFinite(loadedDuration) ? loadedDuration : 0;
        isReady = duration > 0;
        if (videoElement.videoWidth && videoElement.videoHeight) {
            videoAspectRatio = videoElement.videoWidth / videoElement.videoHeight;
        }
        currentTime = 0;
        videoElement.currentTime = 0;
        nextSegmentId = 1;
        
        initializeSegmentsFromSavedState();
        
        notifySegmentsChange();
        updateThumbnails();
    }

    function handleTimeUpdate() {
        if (isDragging) return;
        currentTime = videoElement.currentTime;
    }

    function handlePlayPause() {
        if (isPlaying) {
            videoElement.pause();
        } else {
            videoElement.play();
        }
        isPlaying = !isPlaying;
    }

    function seekTo(time: number) {
        videoElement.currentTime = Math.max(0, Math.min(time, duration));
    }

    function pauseVideo() {
        if (videoElement && !videoElement.paused) {
            videoElement.pause();
        }
        isPlaying = false;
    }

    function handleTimelineMouseDown(event: MouseEvent) {
        if (!isReady) return;
        wasPlayingBeforeDrag = isPlaying;
        isDragging = true;
        pauseVideo();
        const time = updateCurrentTimeFromMouse(event);
        if (time !== undefined) {
            seekTo(time);
        }
    }

    function handleMouseMove(event: MouseEvent) {
        if (!isDragging || !isReady || !timelineContainer) return;
        updateCurrentTimeFromMouse(event);
    }

    function handleMouseUp() {
        if (!isDragging || !isReady) return;
        isDragging = false;
        seekTo(currentTime);
        if (wasPlayingBeforeDrag) {
            videoElement?.play();
            isPlaying = true;
        }
        wasPlayingBeforeDrag = false;
    }

    function updateCurrentTimeFromMouse(event: MouseEvent) {
        const time = calculateTimeFromMouse(event);
        if (time === null) return;
        currentTime = time;
        return time;
    }

    function calculateTimeFromMouse(event: MouseEvent) {
        if (!timelineContainer) return null;
        const rect = timelineContainer.getBoundingClientRect();
        const x = event.clientX - rect.left;
        const rawTime = (x / rect.width) * duration;
        return Math.max(0, Math.min(rawTime, duration));
    }

    function handleTimelineClick(event: MouseEvent) {
        if (!isReady) return;
        const time = updateCurrentTimeFromMouse(event);
        if (time !== undefined) {
            seekTo(time);
        }
    }

    function cutAtCurrentTime() {
        if (!isReady || currentTime <= 0 || currentTime >= duration) return;
        
        // Find which segment contains the current time
        const segmentIndex = segments.findIndex(seg => 
            currentTime > seg.start && currentTime < seg.end
        );
        
        if (segmentIndex === -1) return;
        
        const segment = segments[segmentIndex];
        
        // Split the segment at current time
        const leftSegment = createSegmentRange(segment.start, currentTime);
        
        const rightSegment = createSegmentRange(currentTime, segment.end);
        
        // Replace the original segment with the two new segments
        segments = [
            ...segments.slice(0, segmentIndex),
            leftSegment,
            rightSegment,
            ...segments.slice(segmentIndex + 1)
        ];
        
        notifySegmentsChange();
    }

    function deleteSegment(segmentId: string) {
        segments = segments.filter(s => s.id !== segmentId);
        notifySegmentsChange();
    }

    function resetSegments() {
        nextSegmentId = 1;
        segments = [createSegmentRange(0, duration)];
        notifySegmentsChange();
    }

    function notifySegmentsChange() {
        const sortedSegments = segments
            .sort((a, b) => a.start - b.start)
            .map(s => ({ start: s.start, end: s.end }));
        onSegmentsChange(sortedSegments);
    }

    function handleRemoveClick() {
        if (onRemoveVideo) {
            onRemoveVideo();
        }
    }

    function handleKeyDown(event: KeyboardEvent) {
        if (!isReady) return;
        
        // Don't trigger shortcuts if user is typing in an input
        if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) {
            return;
        }

        switch (event.code) {
            case 'Space':
                event.preventDefault();
                handlePlayPause();
                break;
            case 'KeyC':
                event.preventDefault();
                cutAtCurrentTime();
                break;
            case 'KeyR':
                event.preventDefault();
                resetSegments();
                break;
        }
    }

        function formatTime(seconds: number): string {
            const mins = Math.floor(seconds / 60);
            const secs = Math.floor(seconds % 60);
            const ms = Math.floor((seconds % 1) * 10);
            return `${mins}:${secs.toString().padStart(2, '0')}.${ms}`;
        }

        function formatMarkerTime(seconds: number): string {
            if (!Number.isFinite(seconds) || seconds < 0) {
                return '0:00';
            }
            const mins = Math.floor(seconds / 60);
            const secs = Math.floor(seconds % 60);
            return `${mins}:${secs.toString().padStart(2, '0')}`;
        }

    function updateThumbnails() {
        // Generate thumbnails for timeline (simplified version)
        // In production, you might want to generate multiple thumbnails
        if (!canvasElement || !videoElement) return;
        
        const ctx = canvasElement.getContext('2d');
        if (!ctx) return;
        
        canvasElement.width = 800;
        canvasElement.height = 60;
        
        // Draw video frame to canvas
        videoElement.currentTime = duration / 2;
        videoElement.addEventListener('seeked', () => {
            ctx.drawImage(videoElement, 0, 0, canvasElement.width, canvasElement.height);
        }, { once: true });
    }

    $: totalDuration = segments.reduce((sum, seg) => sum + (seg.end - seg.start), 0);
</script>

<div class="video-editor">
    <div class="editor-header">
        <div class="editor-header-text">
            <h3 class="editor-title">// video_editor</h3>
            <div class="editor-info">
                <span>Total: {formatTime(totalDuration)}</span>
                <span>•</span>
                <span>{segments.length} segment{segments.length !== 1 ? 's' : ''}</span>
            </div>
        </div>

        <button
            type="button"
            class="editor-remove-btn"
            on:click={handleRemoveClick}
            aria-label="Remove video"
        >
            <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="10"></circle>
                <line x1="15" y1="9" x2="9" y2="15"></line>
                <line x1="9" y1="9" x2="15" y2="15"></line>
            </svg>
            <span>remove video</span>
        </button>
    </div>

    <!-- Video Preview -->
    <div class="video-preview">
        <div class="video-frame" style={`--video-aspect: ${videoAspectRatio}`}>
            <!-- svelte-ignore a11y-media-has-caption -->
            <video
                bind:this={videoElement}
                on:loadedmetadata={handleLoadedMetadata}
                on:timeupdate={handleTimeUpdate}
                on:play={() => isPlaying = true}
                on:pause={() => isPlaying = false}
                class="editor-video"
            ></video>
        </div>
        
        <div class="video-controls">
            <button 
                type="button"
                on:click={handlePlayPause} 
                disabled={!isReady}
                class="control-btn"
                title="Play/Pause (Space)"
            >
                {#if isPlaying}
                    <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <rect x="6" y="4" width="4" height="16"></rect>
                        <rect x="14" y="4" width="4" height="16"></rect>
                    </svg>
                {:else}
                    <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <polygon points="5 3 19 12 5 21 5 3"></polygon>
                    </svg>
                {/if}
            </button>
            
            <div class="time-display">
                {formatTime(currentTime)} / {formatTime(duration)}
            </div>
            
            <button 
                type="button"
                on:click={cutAtCurrentTime}
                disabled={!isReady || currentTime <= 0 || currentTime >= duration}
                class="control-btn cut-btn"
                title="Cut at current time (C)"
            >
                <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <circle cx="6" cy="6" r="3"></circle>
                    <circle cx="6" cy="18" r="3"></circle>
                    <line x1="20" y1="4" x2="8.12" y2="15.88"></line>
                    <line x1="14.47" y1="14.48" x2="20" y2="20"></line>
                    <line x1="8.12" y1="8.12" x2="12" y2="12"></line>
                </svg>
            </button>
            
            <button 
                type="button"
                on:click={resetSegments}
                disabled={!isReady}
                class="control-btn"
                title="Reset to full video (R)"
            >
                <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <polyline points="23 4 23 10 17 10"></polyline>
                    <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
                </svg>
            </button>
        </div>
    </div>

    <!-- Timeline -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="timeline-section">
        <div class="timeline-help">
            // Space: play/pause • C: cut • R: reset • Drag scrubber to seek
        </div>
        
        <div 
            class="timeline-container"
            class:dragging={isDragging}
            bind:this={timelineContainer}
            on:click={handleTimelineClick}
            on:mousedown={handleTimelineMouseDown}
            role="slider"
            tabindex="0"
            aria-label="Video timeline"
            aria-valuenow="{currentTime}"
            aria-valuemin="0"
            aria-valuemax="{duration}"
        >
            <!-- Background canvas for thumbnails -->
            <canvas bind:this={canvasElement} class="timeline-canvas"></canvas>
            
            <!-- Current time indicator -->
            {#if isReady}
                <div 
                    class="timeline-cursor" 
                    style="left: {(currentTime / duration) * 100}%"
                ></div>
            {/if}
            
            <!-- Segments -->
            {#each segments as segment (segment.id)}
                <div 
                    class="timeline-segment"
                    style="left: {(segment.start / duration) * 100}%; width: {((segment.end - segment.start) / duration) * 100}%"
                >
                    <div class="segment-label">
                        {formatTime(segment.start)} - {formatTime(segment.end)}
                    </div>
                    <button 
                        type="button"
                        class="segment-delete"
                        on:click|stopPropagation={() => deleteSegment(segment.id)}
                        title="Delete segment"
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                            <line x1="18" y1="6" x2="6" y2="18"></line>
                            <line x1="6" y1="6" x2="18" y2="18"></line>
                        </svg>
                    </button>
                </div>
            {/each}
        </div>
        
        <!-- Time markers -->
        <div class="timeline-markers">
            {#each Array(11) as _, i}
                <div 
                    class="time-marker" 
                    class:start-marker={i === 0}
                    class:end-marker={i === 10}
                    style={i === 10 ? 'right: 0;' : `left: ${i * 10}%;`}
                >
                    {formatMarkerTime((duration * i) / 10)}
                </div>
            {/each}
        </div>
    </div>

    <!-- Segment List -->
    <div class="segments-list">
        <h4 class="segments-title">// segments</h4>
        <div class="segments-items">
            {#each segments as segment, idx (segment.id)}
                <div class="segment-item">
                    <span class="segment-number">{idx + 1}</span>
                    <span class="segment-time">
                        {formatTime(segment.start)} → {formatTime(segment.end)}
                    </span>
                    <span class="segment-duration">
                        ({formatTime(segment.end - segment.start)})
                    </span>
                    <button 
                        type="button"
                        class="segment-item-delete"
                        on:click={() => deleteSegment(segment.id)}
                        aria-label="Delete segment {idx + 1}"
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                            <polyline points="3 6 5 6 21 6"></polyline>
                            <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                        </svg>
                    </button>
                </div>
            {/each}
        </div>
    </div>
</div>

<style>
    .video-editor {
        width: 100%;
    }

    .editor-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 1rem;
    }

    .editor-header-text {
        display: flex;
        flex-direction: column;
        gap: 0.25rem;
    }

    .editor-title {
        font-size: 1rem;
        color: #fafafa;
        font-weight: 600;
        margin: 0;
        text-transform: lowercase;
    }

    .editor-info {
        display: flex;
        gap: 0.5rem;
        font-size: 0.9rem;
        color: #71717a;
    }

    .editor-remove-btn {
        display: inline-flex;
        gap: 6px;
        align-items: center;
        padding: 6px 10px;
        border-radius: 2px;
        background: rgba(220, 38, 38, 0.9);
        border: 1px solid rgba(185, 28, 28, 0.8);
        color: #fafafa;
        font-size: 12px;
        font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
        font-weight: 500;
        cursor: pointer;
        transition: all 0.2s ease;
        outline: none;
        -webkit-tap-highlight-color: transparent;
        touch-action: manipulation;
    }

    .editor-remove-btn:hover {
        background: rgba(185, 28, 28, 0.95);
        transform: translateY(-1px);
    }

    .editor-remove-btn:active {
        transform: translateY(0);
    }

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
    }

    .editor-video {
        width: 100%;
        height: 100%;
        display: block;
        object-fit: contain;
        background: #09090b;
    }

    .video-controls {
        display: flex;
        align-items: center;
        gap: 1rem;
        padding: 0.75rem 1rem;
        background: rgba(9, 9, 11, 0.8);
        border-top: 1px solid #27272a;
    }

    .control-btn {
        background: rgba(255, 255, 255, 0.1);
        border: 1px solid rgba(255, 255, 255, 0.2);
        color: #fafafa;
        padding: 0.5rem;
        border-radius: 2px;
        cursor: pointer;
        display: flex;
        align-items: center;
        justify-content: center;
        transition: all 0.2s;
        outline: none;
        -webkit-tap-highlight-color: transparent;
    }

    .control-btn:hover:not(:disabled) {
        background: rgba(24, 24, 27, 0.8);
        border-color: #3f3f46;
    }

    .control-btn:disabled {
        opacity: 0.3;
        cursor: not-allowed;
    }

    .cut-btn:not(:disabled) {
        background: rgba(24, 24, 27, 0.8);
        border-color: #3f3f46;
    }

    .cut-btn:hover:not(:disabled) {
        background: rgba(39, 39, 42, 0.8);
        border-color: #52525b;
    }

    .time-display {
        font-family: 'Courier New', monospace;
        font-size: 0.9rem;
        color: #a1a1aa;
        flex: 0 0 auto;
        white-space: nowrap;
        min-width: fit-content;
    }

    .timeline-section {
        margin-bottom: 1.5rem;
    }

    .timeline-help {
        font-size: 0.85rem;
        color: #71717a;
        margin-bottom: 0.5rem;
    }

    .timeline-container {
        position: relative;
        width: 100%;
        height: 80px;
        background: rgba(9, 9, 11, 0.6);
        border: 1px solid #27272a;
        border-radius: 4px;
        cursor: pointer;
        overflow: hidden;
        user-select: none;
    }

    .timeline-container.dragging {
        cursor: grabbing;
    }

    .timeline-canvas {
        position: absolute;
        top: 0;
        left: 0;
        width: 100%;
        height: 100%;
        opacity: 0.3;
        pointer-events: none;
    }

    .timeline-cursor {
        position: absolute;
        top: 0;
        width: 2px;
        height: 100%;
        background: #fafafa;
        pointer-events: none;
        z-index: 10;
    }

    .timeline-segment {
        position: absolute;
        top: 0;
        height: 100%;
        background: rgba(250, 250, 250, 0.1);
        border: 1px solid #3f3f46;
        transition: background 0.2s;
    }

    .timeline-segment:hover {
        background: rgba(250, 250, 250, 0.15);
        border-color: #52525b;
    }

    .segment-label {
        position: absolute;
        top: 50%;
        left: 50%;
        transform: translate(-50%, -50%);
        font-size: 0.75rem;
        color: #fafafa;
        font-weight: 600;
        pointer-events: none;
        white-space: nowrap;
        text-shadow: 0 0 4px rgba(0, 0, 0, 0.8);
    }

    .segment-delete {
        position: absolute;
        top: 4px;
        right: 4px;
        background: rgba(220, 38, 38, 0.8);
        border: none;
        color: #fafafa;
        width: 20px;
        height: 20px;
        border-radius: 2px;
        cursor: pointer;
        display: flex;
        align-items: center;
        justify-content: center;
        opacity: 0;
        transition: opacity 0.2s;
    }

    .timeline-segment:hover .segment-delete {
        opacity: 1;
    }

    .segment-delete:hover {
        background: rgba(185, 28, 28, 0.95);
    }

    .timeline-markers {
        display: flex;
        position: relative;
        margin-top: 0.5rem;
        height: 20px;
    }

    .time-marker {
        position: absolute;
        font-size: 0.7rem;
        color: #52525b;
        white-space: nowrap;
        transform: translateX(-50%);
    }

    .time-marker.start-marker {
        left: 0;
        transform: none;
        text-align: left;
    }

    .time-marker.end-marker {
        left: auto;
        right: 0;
        transform: none;
        text-align: right;
    }

    .segments-list {
        background: rgba(9, 9, 11, 0.6);
        border: 1px solid #27272a;
        border-radius: 4px;
        padding: 1rem;
    }

    .segments-title {
        font-size: 0.9rem;
        color: #fafafa;
        margin: 0 0 0.75rem 0;
        font-weight: 600;
        text-transform: lowercase;
    }

    .segments-items {
        display: flex;
        flex-direction: column;
        gap: 0.5rem;
    }

    .segment-item {
        display: flex;
        align-items: center;
        gap: 0.75rem;
        padding: 0.5rem;
        background: rgba(24, 24, 27, 0.4);
        border: 1px solid rgba(39, 39, 42, 0.4);
        border-radius: 2px;
        font-size: 0.85rem;
    }

    .segment-number {
        background: rgba(250, 250, 250, 0.1);
        color: #fafafa;
        width: 24px;
        height: 24px;
        border-radius: 2px;
        display: flex;
        align-items: center;
        justify-content: center;
        font-weight: 600;
        flex-shrink: 0;
        border: 1px solid #3f3f46;
    }

    .segment-time {
        color: #fafafa;
        font-family: 'Courier New', monospace;
        flex: 1;
    }

    .segment-duration {
        color: #71717a;
        font-family: 'Courier New', monospace;
    }

    .segment-item-delete {
        background: none;
        border: none;
        color: #dc2626;
        cursor: pointer;
        padding: 0.25rem;
        display: flex;
        align-items: center;
        justify-content: center;
        opacity: 0.5;
        transition: opacity 0.2s;
    }

    .segment-item-delete:hover {
        opacity: 1;
    }
</style>
