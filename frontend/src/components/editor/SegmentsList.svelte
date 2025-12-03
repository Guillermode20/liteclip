<script lang="ts">
    import { createEventDispatcher } from 'svelte';

    interface Segment {
        start: number;
        end: number;
        id: string;
    }

    export let segments: Segment[] = [];

    const dispatch = createEventDispatcher<{
        delete: { id: string };
    }>();

    function formatTime(seconds: number): string {
        const mins = Math.floor(seconds / 60);
        const secs = Math.floor(seconds % 60);
        const ms = Math.floor((seconds % 1) * 10);
        return `${mins}:${secs.toString().padStart(2, '0')}.${ms}`;
    }
</script>

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
                    on:click={() => dispatch('delete', { id: segment.id })}
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

<style>
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
