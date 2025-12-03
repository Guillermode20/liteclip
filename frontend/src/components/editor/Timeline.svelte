<script lang="ts">
    import { createEventDispatcher } from 'svelte';

    interface Segment {
        start: number;
        end: number;
        id: string;
    }

    export let segments: Segment[] = [];
    export let duration: number = 0;
    export let currentTime: number = 0;
    export let isReady: boolean = false;
    export let isDragging: boolean = false;
    export let visualScrubberPosition: number = 0;

    const dispatch = createEventDispatcher<{
        timelineClick: { time: number };
        mousedown: { event: MouseEvent };
        deleteSegment: { id: string };
    }>();

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

    function handleClick(event: MouseEvent) {
        if (!isReady) return;
        const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
        const x = event.clientX - rect.left;
        const time = (x / rect.width) * duration;
        dispatch('timelineClick', { time: Math.max(0, Math.min(time, duration)) });
    }

    function handleMouseDown(event: MouseEvent) {
        if (!isReady) return;
        dispatch('mousedown', { event });
    }

    function handleDeleteClick(event: MouseEvent, id: string) {
        event.stopPropagation();
        dispatch('deleteSegment', { id });
    }

    $: cursorPosition = (isDragging ? visualScrubberPosition : currentTime) / duration * 100;
</script>

<div class="timeline-section">
    <div class="timeline-help">
        // Space: play/pause • C: cut • R: reset • Click or drag timeline to seek
    </div>
    
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div 
        class="timeline-container"
        class:dragging={isDragging}
        on:click={handleClick}
        on:mousedown={handleMouseDown}
        role="slider"
        tabindex="0"
        aria-label="Video timeline"
        aria-valuenow="{currentTime}"
        aria-valuemin="0"
        aria-valuemax="{duration}"
    >
        <!-- Current time indicator -->
        {#if isReady}
            <div 
                class="timeline-cursor" 
                style="left: {cursorPosition}%"
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
                    on:click={(e) => handleDeleteClick(e, segment.id)}
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

<style>
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
        contain: layout style;
    }

    .timeline-container.dragging {
        cursor: grabbing;
    }

    .timeline-cursor {
        position: absolute;
        top: 0;
        width: 2px;
        height: 100%;
        background: #fafafa;
        pointer-events: none;
        z-index: 10;
        will-change: left;
        transform: translateZ(0);
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
</style>
