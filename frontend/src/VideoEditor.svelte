<script lang="ts">
    import { onDestroy, onMount } from 'svelte';
    import type { VideoSegment } from './types';
    import { loadVideoFile, type VideoMetadata, type VideoLoadError } from './services/videoLoader';
    import { EditorHeader, VideoPreview, Timeline, SegmentsList } from './components/editor';
    import {
        type SegmentWithId,
        initializeSegments,
        cutSegmentAtTime,
        deleteSegment as removeSegment,
        toVideoSegments,
        getTotalDuration,
        resetSegmentCounter
    } from './hooks/useSegments';

    // ============================================================================
    // Props
    // ============================================================================
    export let videoFile: File;
    export let onSegmentsChange: (segments: Array<{start: number, end: number}>) => void;
    export let onRemoveVideo: (() => void) | null = null;
    export let savedSegments: VideoSegment[] = [];
    export let onMetadataLoaded: ((payload: { width: number; height: number; duration: number }) => void) | null = null;
    export let onCropChange: (crop: { x: number; y: number; width: number; height: number } | null) => void;

    // ============================================================================
    // State
    // ============================================================================
    let videoElement: HTMLVideoElement | null = null;
    let duration: number = 0;
    let currentTime: number = 0;
    let visualScrubTime: number = 0;
    let isPlaying: boolean = false;
    let isReady: boolean = false;
    let isLoading: boolean = false;
    let loadError: string | null = null;
    let videoAspectRatio = 16 / 9;
    
    // Crop state
    let crop = { x: 0, y: 0, width: 100, height: 100 };
    let isCropActive = false;

    // Drag state
    let isDragging: boolean = false;
    let wasPlayingBeforeDrag: boolean = false;
    let lastVideoUpdateTime: number = 0;
    let videoSeekTimeout: ReturnType<typeof setTimeout> | null = null;
    let timeUpdateRafId: number | null = null;
    const VIDEO_UPDATE_INTERVAL_MS = 100;
    
    let visualScrubberPosition: number = 0;
    let segments: SegmentWithId[] = [];
    let objectUrl: string | null = null;
    let loadAbortController: AbortController | null = null;
    let fileLoadSequence = 0;
    let prevVideoFile: File | null = null;

    // ============================================================================
    // Derived values
    // ============================================================================
    $: totalDuration = getTotalDuration(segments);
    
    // Reactive reload when videoFile changes
    $: if (videoFile && videoFile !== prevVideoFile) {
        prevVideoFile = videoFile;
        if (typeof window !== 'undefined') {
            loadVideoAsync();
        }
    }

    // ============================================================================
    // Lifecycle
    // ============================================================================
    onMount(() => {
        prevVideoFile = videoFile;
        if (videoFile) {
            loadVideoAsync();
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

    onDestroy(() => {
        cancelPendingLoad();
        cleanupResources();
    });

    // ============================================================================
    // Video Loading
    // ============================================================================
    function cancelPendingLoad() {
        if (loadAbortController) {
            loadAbortController.abort();
            loadAbortController = null;
        }
    }
    
    function cleanupResources() {
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
            objectUrl = null;
        }
        if (timeUpdateRafId) {
            cancelAnimationFrame(timeUpdateRafId);
            timeUpdateRafId = null;
        }
        if (videoSeekTimeout) {
            clearTimeout(videoSeekTimeout);
            videoSeekTimeout = null;
        }
    }

    async function loadVideoAsync() {
        cancelPendingLoad();
        const currentSequence = ++fileLoadSequence;
        
        isReady = false;
        isLoading = true;
        loadError = null;
        duration = 0;
        currentTime = 0;
        
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
            objectUrl = null;
        }
        
        loadAbortController = new AbortController();
        const signal = loadAbortController.signal;
        
        try {
            const result = await loadVideoFile(videoFile, signal);
            
            if (currentSequence !== fileLoadSequence || signal.aborted) {
                URL.revokeObjectURL(result.objectUrl);
                return;
            }
            
            objectUrl = result.objectUrl;
            applyMetadata(result.metadata);
            
            if (videoElement) {
                videoElement.src = objectUrl;
                await waitForVideoElementReady();
            }
            
            isReady = duration > 0;
            isLoading = false;
            
        } catch (error) {
            if (currentSequence !== fileLoadSequence) return;
            
            isLoading = false;
            const loadErr = error as VideoLoadError;
            if (loadErr.type === 'aborted') return;
            
            loadError = loadErr.message || 'Failed to load video';
            console.error('Video load failed:', loadError);
        } finally {
            if (currentSequence === fileLoadSequence) {
                loadAbortController = null;
            }
        }
    }
    
    function waitForVideoElementReady(): Promise<void> {
        return new Promise((resolve) => {
            if (!videoElement) {
                resolve();
                return;
            }
            
            if (videoElement.readyState >= HTMLMediaElement.HAVE_METADATA) {
                resolve();
                return;
            }
            
            const onReady = () => {
                videoElement?.removeEventListener('loadedmetadata', onReady);
                videoElement?.removeEventListener('error', onReady);
                resolve();
            };
            
            videoElement.addEventListener('loadedmetadata', onReady, { once: true });
            videoElement.addEventListener('error', onReady, { once: true });
        });
    }
    
    function applyMetadata(metadata: VideoMetadata) {
        duration = metadata.duration;
        videoAspectRatio = metadata.aspectRatio;
        currentTime = 0;
        resetSegmentCounter();
        
        if (onMetadataLoaded && duration > 0) {
            onMetadataLoaded({
                width: metadata.width,
                height: metadata.height,
                duration: metadata.duration
            });
        }
        
        segments = initializeSegments(savedSegments, duration);
        notifySegmentsChange();
    }

    // ============================================================================
    // Video Playback
    // ============================================================================
    function handleLoadedMetadata() {
        if (!videoElement) return;
        if (videoElement.currentTime !== 0) {
            videoElement.currentTime = 0;
            currentTime = 0;
        }
    }

    function handleTimeUpdate() {
        if (!videoElement) return;
        if (timeUpdateRafId) return;
        
        timeUpdateRafId = requestAnimationFrame(() => {
            if (videoElement) {
                currentTime = videoElement.currentTime;
                if (isDragging) {
                    visualScrubTime = currentTime;
                }
            }
            timeUpdateRafId = null;
        });
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

    // ============================================================================
    // Timeline Interaction
    // ============================================================================
    function handleTimelineClick(event: CustomEvent<{ time: number }>) {
        if (!isReady || !videoElement) return;
        videoElement.currentTime = event.detail.time;
    }

    function handleTimelineMouseDown(event: CustomEvent<{ event: MouseEvent }>) {
        if (!isReady || !videoElement) return;
        wasPlayingBeforeDrag = isPlaying;
        isDragging = true;
        if (isPlaying) {
            videoElement.pause();
        }
        const rect = (event.detail.event.currentTarget as HTMLElement).getBoundingClientRect();
        const x = event.detail.event.clientX - rect.left;
        visualScrubTime = Math.max(0, Math.min((x / rect.width) * duration, duration));
    }

    function handleMouseMove(event: MouseEvent) {
        if (!isDragging || !isReady) return;
        
        const timeline = document.querySelector('.timeline-container');
        if (!timeline) return;
        
        const rect = timeline.getBoundingClientRect();
        const x = event.clientX - rect.left;
        visualScrubTime = Math.max(0, Math.min((x / rect.width) * duration, duration));
        visualScrubberPosition = visualScrubTime;
        
        scheduleVideoSeek();
    }
    
    function scheduleVideoSeek() {
        if (videoSeekTimeout) {
            clearTimeout(videoSeekTimeout);
        }
        
        const now = performance.now();
        if (now - lastVideoUpdateTime >= VIDEO_UPDATE_INTERVAL_MS) {
            performVideoSeek();
        } else {
            videoSeekTimeout = setTimeout(performVideoSeek, VIDEO_UPDATE_INTERVAL_MS);
        }
    }
    
    function performVideoSeek() {
        if (!videoElement || !isDragging) return;
        lastVideoUpdateTime = performance.now();
        videoElement.currentTime = visualScrubTime;
        videoSeekTimeout = null;
    }

    function handleMouseUp() {
        if (!isDragging || !videoElement) return;
        isDragging = false;
        
        if (videoSeekTimeout) {
            clearTimeout(videoSeekTimeout);
            videoSeekTimeout = null;
        }
        
        videoElement.currentTime = visualScrubTime;
        currentTime = visualScrubTime;
        
        if (wasPlayingBeforeDrag) {
            videoElement.play();
            isPlaying = true;
        }
        wasPlayingBeforeDrag = false;
    }

    // ============================================================================
    // Segment Operations
    // ============================================================================
    function cutAtCurrentTime() {
        const cutTime = isDragging ? visualScrubberPosition : currentTime;
        if (!isReady) return;
        
        const newSegments = cutSegmentAtTime(segments, cutTime, duration);
        if (newSegments) {
            segments = newSegments;
            notifySegmentsChange();
        }
    }

    function handleDeleteSegment(event: CustomEvent<{ id: string }>) {
        segments = removeSegment(segments, event.detail.id);
        notifySegmentsChange();
    }

    function resetSegments() {
        resetSegmentCounter();
        segments = initializeSegments(null, duration);
        notifySegmentsChange();
    }

    function notifySegmentsChange() {
        onSegmentsChange(toVideoSegments(segments));
    }

    function notifyCropChange() {
        onCropChange(crop);
    }

    // ============================================================================
    // Keyboard Shortcuts
    // ============================================================================
    function handleKeyDown(event: KeyboardEvent) {
        if (!isReady) return;
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
</script>

<div class="video-editor">
    <EditorHeader
        {totalDuration}
        segmentCount={segments.length}
        {isCropActive}
        on:remove={() => onRemoveVideo?.()}
        on:toggleCrop={() => isCropActive = !isCropActive}
    />

    <VideoPreview
        bind:videoElement
        {videoAspectRatio}
        {isPlaying}
        {isReady}
        {currentTime}
        {duration}
        {isDragging}
        {visualScrubberPosition}
        {crop}
        {isCropActive}
        on:loadedmetadata={handleLoadedMetadata}
        on:timeupdate={handleTimeUpdate}
        on:play={() => isPlaying = true}
        on:pause={() => isPlaying = false}
        on:playPause={handlePlayPause}
        on:cut={cutAtCurrentTime}
        on:reset={resetSegments}
        on:cropChange={(e) => {
            crop = e.detail;
            notifyCropChange();
        }}
    />

    <Timeline
        {segments}
        {duration}
        {currentTime}
        {isReady}
        {isDragging}
        {visualScrubberPosition}
        on:timelineClick={handleTimelineClick}
        on:mousedown={handleTimelineMouseDown}
        on:deleteSegment={handleDeleteSegment}
    />

    <SegmentsList
        {segments}
        on:delete={handleDeleteSegment}
    />
</div>

<style>
    .video-editor {
        width: 100%;
    }
</style>
