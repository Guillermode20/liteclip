<script lang="ts">
    import { createEventDispatcher } from 'svelte';

    export let isPlaying: boolean = false;
    export let isReady: boolean = false;
    export let currentTime: number = 0;
    export let duration: number = 0;
    export let isDragging: boolean = false;
    export let visualScrubberPosition: number = 0;

    const dispatch = createEventDispatcher<{
        playPause: void;
        cut: void;
        reset: void;
    }>();

    function formatTime(seconds: number): string {
        const mins = Math.floor(seconds / 60);
        const secs = Math.floor(seconds % 60);
        const ms = Math.floor((seconds % 1) * 10);
        return `${mins}:${secs.toString().padStart(2, '0')}.${ms}`;
    }

    $: displayTime = isDragging ? visualScrubberPosition : currentTime;
    $: canCut = isReady && displayTime > 0 && displayTime < duration;
</script>

<div class="simple-controls">
    <button 
        type="button"
        on:click={() => dispatch('playPause')} 
        disabled={!isReady}
        class="simple-btn play-btn"
        title="Play/Pause (Space)"
    >
        {#if isPlaying}
            <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="6" y="4" width="4" height="16"></rect>
                <rect x="14" y="4" width="4" height="16"></rect>
            </svg>
            Pause
        {:else}
            <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polygon points="5 3 19 12 5 21 5 3"></polygon>
            </svg>
            Play
        {/if}
    </button>
    
    <div class="time-display">
        {formatTime(displayTime)} / {formatTime(duration)}
    </div>
    
    <button 
        type="button"
        on:click={() => dispatch('cut')}
        disabled={!canCut}
        class="simple-btn"
        title="Cut at current time (C)"
    >
        Cut at {formatTime(displayTime)}
    </button>
    
    <button 
        type="button"
        on:click={() => dispatch('reset')}
        disabled={!isReady}
        class="simple-btn"
        title="Reset to full video (R)"
    >
        Reset
    </button>
</div>

<style>
    .simple-controls {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        padding: 0.75rem 1rem;
        background: rgba(9, 9, 11, 0.8);
        border-top: 1px solid #27272a;
    }

    .simple-btn {
        background: rgba(255, 255, 255, 0.1);
        border: 1px solid rgba(255, 255, 255, 0.2);
        color: #fafafa;
        padding: 0.5rem 1rem;
        border-radius: 2px;
        cursor: pointer;
        font-size: 0.85rem;
        transition: all 0.2s;
        outline: none;
        display: flex;
        align-items: center;
        gap: 0.5rem;
    }

    .play-btn {
        padding: 0.5rem 0.75rem;
    }

    .simple-btn:hover:not(:disabled) {
        background: rgba(24, 24, 27, 0.8);
        border-color: #3f3f46;
    }

    .simple-btn:disabled {
        opacity: 0.3;
        cursor: not-allowed;
    }

    .time-display {
        font-family: 'Courier New', monospace;
        font-size: 0.9rem;
        color: #a1a1aa;
        flex: 0 0 auto;
        white-space: nowrap;
        min-width: fit-content;
    }
</style>
