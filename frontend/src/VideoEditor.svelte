<script lang="ts">
    import { onDestroy, onMount } from 'svelte';
    import type { VideoSegment } from './types';

    export let videoFile: File;
    export let onSegmentsChange: (segments: Array<{start: number, end: number}>) => void;
    export let onRemoveVideo: (() => void) | null = null;
    export let savedSegments: VideoSegment[] = [];
    export let onMetadataLoaded: ((payload: { width: number; height: number; duration: number }) => void) | null = null;

    let videoElement: HTMLVideoElement | null = null;
    let canvasElement: HTMLCanvasElement | null = null;
    // Low-res preview canvas for scrubbing
    let previewCanvas: HTMLCanvasElement | null = null;
    let timelineContainer: HTMLDivElement | null = null;
    
    let duration: number = 0;
    let currentTime: number = 0;
    let isPlaying: boolean = false;
    let isReady: boolean = false;
    let isDragging: boolean = false;
    let wasPlayingBeforeDrag: boolean = false;
    let videoAspectRatio = 16 / 9;
    const MAX_PREVIEW_WIDTH = 1280; // cap preview to 720p
    const MAX_PREVIEW_HEIGHT = 720;

    // Preview canvas context and sizes
    let previewCtx: CanvasRenderingContext2D | null = null;
    let previewWidth = 0;
    let previewHeight = 0;

    // Scrubbing optimization: throttle interval (ms)
    const SCRUB_THROTTLE_MS = 8; // ~120fps for smooth scrubbing
    let lastScrubTime = 0;
    let scrubAnimationFrame: number | null = null;

    // Timeline thumbnail strip
    const TIMELINE_THUMBNAIL_COUNT = 10;
    let timelineThumbnailsGenerated = false;

    // === LIVE SCRUB PREVIEW SYSTEM ===
    // Pre-generated frames for instant scrub preview (adaptive frame count based on duration)
    const SCRUB_FRAME_INTERVAL = 0.25; // seconds between scrub frames
    const MAX_SCRUB_FRAMES = 300; // Increased to support longer videos (75 seconds at 0.25s intervals)
    const scrubFrames = new Map<number, ImageBitmap>(); // time -> ImageBitmap (GPU-optimized)
    let scrubFramesGenerated = false;
    let scrubFrameGenerationInProgress = false;
    let scrubFrameAbortController: AbortController | null = null;
    
    // Offscreen canvas for frame extraction (doesn't touch DOM)
    let offscreenCanvas: OffscreenCanvas | null = null;
    let offscreenCtx: OffscreenCanvasRenderingContext2D | null = null;
    
    // Trim segments (kept segments)
    let segments: Array<{start: number, end: number, id: string}> = [];
    let nextSegmentId = 1;
    let objectUrl: string | null = null;
    
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
        // Pointer events support (touch/stylus); keep mouse as fallback
        window.addEventListener('pointerup', handleMouseUp);
        window.addEventListener('pointermove', handleMouseMove);
        return () => {
            window.removeEventListener('keydown', handleKeyDown);
            window.removeEventListener('mouseup', handleMouseUp);
            window.removeEventListener('mousemove', handleMouseMove);
            window.removeEventListener('pointerup', handleMouseUp);
            window.removeEventListener('pointermove', handleMouseMove);
        };
    });

    onDestroy(() => {
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
            objectUrl = null;
        }
        // Clean up animation frame and caches
        if (scrubAnimationFrame !== null) {
            cancelAnimationFrame(scrubAnimationFrame);
            scrubAnimationFrame = null;
        }
        clearScrubFrames();
    });

    function loadVideo() {
        if (!videoElement) return;
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
        }
        // Reset state for new video
        timelineThumbnailsGenerated = false;
        clearScrubFrames();
        
        objectUrl = URL.createObjectURL(videoFile);
        videoElement.src = objectUrl;
        videoElement.load();
    }

    function handleLoadedMetadata() {
        if (!videoElement) return;
        const loadedDuration = videoElement.duration;
        duration = Number.isFinite(loadedDuration) ? loadedDuration : 0;
        isReady = duration > 0;
        const width = videoElement.videoWidth || 0;
        const height = videoElement.videoHeight || 0;
        if (width && height) {
            videoAspectRatio = width / height;
        }
        currentTime = 0;
        videoElement.currentTime = 0;
        nextSegmentId = 1;
        
        if (onMetadataLoaded && duration > 0) {
            onMetadataLoaded({ width, height, duration });
        }
        
        initializeSegmentsFromSavedState();
        
        notifySegmentsChange();
        updateThumbnails();
        updatePreviewCanvasSize();
        ensurePreviewContext();
        
        // Start generating scrub preview frames in background (non-blocking)
        // This enables instant live preview during scrubbing
        generateScrubFrames();
    }

    function updatePreviewCanvasSize() {
        if (!previewCanvas || !videoElement) return;
        const videoW = videoElement.videoWidth || 1280;
        const videoH = videoElement.videoHeight || 720;
        const aspect = videoW / videoH;
        let targetW = Math.min(videoW, MAX_PREVIEW_WIDTH);
        let targetH = Math.round(targetW / aspect);
        if (targetH > MAX_PREVIEW_HEIGHT) {
            targetH = MAX_PREVIEW_HEIGHT;
            targetW = Math.round(targetH * aspect);
        }
        previewWidth = targetW;
        previewHeight = targetH;
        previewCanvas.width = previewWidth;
        previewCanvas.height = previewHeight;
        // scale to fill the frame; keep internal resolution lower than full-res
        previewCanvas.style.width = `100%`;
        previewCanvas.style.height = `100%`;
    }

    function ensurePreviewContext() {
        if (!previewCanvas) return;
        previewCtx = previewCanvas.getContext('2d');
        if (previewCtx) {
            previewCtx.imageSmoothingEnabled = true;
            (previewCtx as any).imageSmoothingQuality = 'high';
        }
    }

    // === LIVE SCRUB PREVIEW FUNCTIONS ===
    
    /** Clear all pre-generated scrub frames and abort any in-progress generation */
    function clearScrubFrames() {
        // Abort any in-progress generation
        if (scrubFrameAbortController) {
            scrubFrameAbortController.abort();
            scrubFrameAbortController = null;
        }
        // Close all ImageBitmaps to free GPU memory
        for (const bitmap of scrubFrames.values()) {
            bitmap.close();
        }
        scrubFrames.clear();
        scrubFramesGenerated = false;
        scrubFrameGenerationInProgress = false;
    }
    
    /** Get the nearest pre-generated scrub frame for a given time */
    function getNearestScrubFrame(time: number): ImageBitmap | null {
        if (scrubFrames.size === 0) return null;
        
        // Find nearest available frame (since interval is now dynamic)
        let nearestTime = 0;
        let nearestDist = Infinity;
        for (const t of scrubFrames.keys()) {
            const dist = Math.abs(t - time);
            if (dist < nearestDist) {
                nearestDist = dist;
                nearestTime = t;
            }
        }
        
        return scrubFrames.get(nearestTime) ?? null;
    }
    
    /** Generate scrub preview frames in the background (non-blocking) */
    async function generateScrubFrames() {
        if (scrubFrameGenerationInProgress || scrubFramesGenerated) return;
        if (!videoElement || !duration || duration <= 0) return;
        
        scrubFrameGenerationInProgress = true;
        scrubFrameAbortController = new AbortController();
        const signal = scrubFrameAbortController.signal;
        
        // Calculate how many frames to generate - now adaptive to video duration
        // For longer videos, increase interval to keep frame count reasonable
        let frameInterval = SCRUB_FRAME_INTERVAL;
        let frameCount = Math.ceil(duration / frameInterval);
        
        // If video is very long, increase interval to keep frames manageable
        if (frameCount > MAX_SCRUB_FRAMES) {
            frameInterval = duration / MAX_SCRUB_FRAMES;
            frameCount = MAX_SCRUB_FRAMES;
        }
        
        const times: number[] = [];
        for (let i = 0; i < frameCount; i++) {
            times.push(i * frameInterval);
        }
        
        // Create a hidden video element for frame extraction (doesn't affect main video)
        const extractionVideo = document.createElement('video');
        extractionVideo.muted = true;
        extractionVideo.preload = 'auto';
        extractionVideo.src = objectUrl!;
        
        // Wait for video to be ready
        await new Promise<void>((resolve, reject) => {
            extractionVideo.onloadeddata = () => resolve();
            extractionVideo.onerror = () => reject(new Error('Failed to load video for frame extraction'));
            signal.addEventListener('abort', () => reject(new Error('Aborted')));
        }).catch(() => {
            scrubFrameGenerationInProgress = false;
            return;
        });
        
        if (signal.aborted) {
            scrubFrameGenerationInProgress = false;
            return;
        }
        
        // Create offscreen canvas for frame extraction
        const frameWidth = Math.min(640, extractionVideo.videoWidth || 640);
        const frameHeight = Math.round(frameWidth / (extractionVideo.videoWidth / extractionVideo.videoHeight || 16/9));
        
        if (typeof OffscreenCanvas !== 'undefined') {
            offscreenCanvas = new OffscreenCanvas(frameWidth, frameHeight);
            offscreenCtx = offscreenCanvas.getContext('2d');
        }
        
        // Generate frames one at a time with yielding to keep UI responsive
        for (let i = 0; i < times.length; i++) {
            if (signal.aborted) break;
            
            const time = times[i];
            
            try {
                // Seek to frame time
                extractionVideo.currentTime = time;
                await new Promise<void>((resolve) => {
                    const onSeeked = () => {
                        extractionVideo.removeEventListener('seeked', onSeeked);
                        resolve();
                    };
                    extractionVideo.addEventListener('seeked', onSeeked);
                });
                
                if (signal.aborted) break;
                
                // Create ImageBitmap from video frame (GPU-accelerated)
                const bitmap = await createImageBitmap(extractionVideo, {
                    resizeWidth: frameWidth,
                    resizeHeight: frameHeight,
                    resizeQuality: 'low' // Fast resize for scrubbing
                });
                
                scrubFrames.set(time, bitmap);
                
                // Yield every 5 frames to keep UI responsive
                if (i % 5 === 0) {
                    await new Promise(r => setTimeout(r, 0));
                }
            } catch (e) {
                // Skip failed frames
                continue;
            }
        }
        
        // Cleanup
        extractionVideo.src = '';
        extractionVideo.load();
        
        scrubFrameGenerationInProgress = false;
        if (!signal.aborted) {
            scrubFramesGenerated = true;
        }
    }
    
    /** Draw the live scrub preview frame - INSTANT using pre-generated frames */
    function drawPreviewFrame() {
        if (!previewCanvas || !previewCtx) return;
        
        // Try to get pre-generated scrub frame (instant)
        const scrubFrame = getNearestScrubFrame(currentTime);
        if (scrubFrame) {
            try {
                previewCtx.clearRect(0, 0, previewCanvas.width, previewCanvas.height);
                previewCtx.drawImage(scrubFrame, 0, 0, previewCanvas.width, previewCanvas.height);
                return;
            } catch (e) {
                // Fall through to video element fallback
            }
        }
        
        // Fallback: draw from video element (may show stale frame if video hasn't seeked)
        if (videoElement) {
            try {
                previewCtx.clearRect(0, 0, previewCanvas.width, previewCanvas.height);
                previewCtx.drawImage(videoElement, 0, 0, previewCanvas.width, previewCanvas.height);
            } catch (e) {
                // ignore
            }
        }
    }

    function handleTimeUpdate() {
        if (!videoElement) return;
        // When scrubbing, ignore timeupdate events unless the video was already playing
        // before the drag started — in that case allow updates so playback continues.
        if (isDragging && !wasPlayingBeforeDrag) return;
        currentTime = videoElement.currentTime;
    }

    function handlePlayPause() {
        if (!videoElement) return;
        if (isPlaying) {
            videoElement.pause();
        } else {
            videoElement.play();
        }
        isPlaying = !isPlaying;
    }

    function seekTo(time: number) {
        if (!videoElement) return;
        videoElement.currentTime = Math.max(0, Math.min(time, duration));
    }

    function pauseVideo() {
        if (!videoElement) return;
        if (!videoElement.paused) {
            videoElement.pause();
        }
        isPlaying = false;
    }

    function handleTimelineMouseDown(event: PointerEvent | MouseEvent) {
        if (!isReady || !videoElement) return;
        wasPlayingBeforeDrag = isPlaying;
        isDragging = true;
        if (!isPlaying) pauseVideo();
        if (previewCanvas) previewCanvas.style.display = 'block';
        if (videoElement) videoElement.style.opacity = '0';
        // Update position and draw preview immediately for instant feedback
        updateCurrentTimeFromMouse(event);
        drawPreviewFrame();
    }

    function handleMouseMove(event: PointerEvent | MouseEvent) {
        if (!isDragging || !isReady || !timelineContainer) return;

        // Throttle scrubbing updates for performance
        const now = performance.now();
        if (now - lastScrubTime < SCRUB_THROTTLE_MS) {
            // Schedule an update for the end of the throttle window
            if (scrubAnimationFrame === null) {
                scrubAnimationFrame = requestAnimationFrame(() => {
                    scrubAnimationFrame = null;
                    if (isDragging) {
                        updateCurrentTimeFromMouse(event);
                        drawPreviewFrame();
                    }
                });
            }
            return;
        }
        lastScrubTime = now;

        // Update the scrubber position and draw preview
        updateCurrentTimeFromMouse(event);
        drawPreviewFrame();
    }

    function handleMouseUp() {
        if (!isDragging || !isReady || !videoElement) return;
        isDragging = false;

        // Cancel any pending animation frame
        if (scrubAnimationFrame !== null) {
            cancelAnimationFrame(scrubAnimationFrame);
            scrubAnimationFrame = null;
        }

        // Perform a single seek to the final scrub position
        seekTo(currentTime);
        // Hide preview canvas and restore native video opacity
        if (previewCanvas) previewCanvas.style.display = 'none';
        if (videoElement) videoElement.style.opacity = '';

        if (wasPlayingBeforeDrag) {
            videoElement?.play();
            isPlaying = true;
        }
        wasPlayingBeforeDrag = false;
    }

    function updateCurrentTimeFromMouse(event: PointerEvent | MouseEvent) {
        const time = calculateTimeFromMouse(event);
        if (time === null) return;
        currentTime = time;
        return time;
    }

    function calculateTimeFromMouse(event: PointerEvent | MouseEvent) {
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
        // Generate multiple thumbnails across the timeline
        if (!canvasElement || !videoElement || !duration || duration <= 0) return;
        if (timelineThumbnailsGenerated) return; // Only generate once per video
        
        const ctx = canvasElement.getContext('2d');
        if (!ctx) return;
        
        const thumbWidth = 80;
        const thumbHeight = 60;
        canvasElement.width = thumbWidth * TIMELINE_THUMBNAIL_COUNT;
        canvasElement.height = thumbHeight;
        
        const videoRef = videoElement;
        const canvasRef = canvasElement;
        const prevTime = videoRef.currentTime || 0;
        const wasPlaying = !videoRef.paused;
        
        // Calculate times for each thumbnail
        const times: number[] = [];
        for (let i = 0; i < TIMELINE_THUMBNAIL_COUNT; i++) {
            // Distribute thumbnails evenly, avoiding exact 0 and duration
            const t = ((i + 0.5) / TIMELINE_THUMBNAIL_COUNT) * duration;
            times.push(Math.max(0.1, Math.min(t, duration - 0.1)));
        }
        
        let currentIndex = 0;
        
        const restoreState = () => {
            timelineThumbnailsGenerated = true;
            try {
                videoRef.currentTime = prevTime;
            } catch (e) {
                // ignore
            }
            if (wasPlaying) videoRef.play();
        };
        
        const drawNextThumbnail = () => {
            if (currentIndex >= TIMELINE_THUMBNAIL_COUNT) {
                restoreState();
                return;
            }
            
            const ctx2 = canvasRef.getContext('2d');
            if (!ctx2) {
                restoreState();
                return;
            }
            
            try {
                // Draw the current frame at the appropriate position
                const xOffset = currentIndex * thumbWidth;
                ctx2.drawImage(videoRef, xOffset, 0, thumbWidth, thumbHeight);
            } catch (err) {
                // ignore draw errors
            }
            
            currentIndex++;
            
            if (currentIndex < TIMELINE_THUMBNAIL_COUNT) {
                // Seek to next time
                videoRef.addEventListener('seeked', drawNextThumbnail, { once: true });
                try {
                    videoRef.currentTime = times[currentIndex];
                } catch (e) {
                    // If seeking fails, skip to restore
                    restoreState();
                }
            } else {
                restoreState();
            }
        };
        
        // Start the thumbnail generation chain
        videoRef.addEventListener('seeked', drawNextThumbnail, { once: true });
        try {
            videoRef.currentTime = times[0];
        } catch (e) {
            // If initial seek fails, abort
            restoreState();
        }
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
                <!-- Preview overlay; hidden by default and shown while scrubbing -->
                <canvas bind:this={previewCanvas} class="preview-canvas" style="display:none; pointer-events: none;"></canvas>
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
            on:pointerdown={handleTimelineMouseDown}
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

    /* Limit displayed video preview to 720p maximum to reduce rendering cost */
    .video-frame {
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

    .preview-canvas {
        position: absolute;
        top: 0;
        left: 0;
        width: 100%;
        height: 100%;
        object-fit: contain;
        z-index: 6;
        pointer-events: none;
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
