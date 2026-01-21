<script lang="ts">
    import { createEventDispatcher } from 'svelte';

    export let totalDuration: number = 0;
    export let segmentCount: number = 0;
    export let isCropActive: boolean = false;

    const dispatch = createEventDispatcher<{ 
        remove: void;
        toggleCrop: void;
    }>();

    function formatTime(seconds: number): string {
        const mins = Math.floor(seconds / 60);
        const secs = Math.floor(seconds % 60);
        const ms = Math.floor((seconds % 1) * 10);
        return `${mins}:${secs.toString().padStart(2, '0')}.${ms}`;
    }
</script>

<div class="editor-header">
    <div class="editor-header-text">
        <h3 class="editor-title">// video_editor</h3>
        <div class="editor-info">
            <span>Total: {formatTime(totalDuration)}</span>
            <span>•</span>
            <span>{segmentCount} segment{segmentCount !== 1 ? 's' : ''}</span>
        </div>
    </div>

    <div class="editor-header-actions">
        <button
            type="button"
            class="editor-crop-btn"
            class:active={isCropActive}
            on:click={() => dispatch('toggleCrop')}
            title={isCropActive ? "Apply crop" : "Crop video"}
        >
            <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M6.13 1L6 16a2 2 0 0 0 2 2h15"></path>
                <path d="M1 6.13L16 6a2 2 0 0 1 2 2v15"></path>
            </svg>
            <span>{isCropActive ? 'done' : 'crop'}</span>
        </button>

        <button
            type="button"
            class="editor-remove-btn"
            on:click={() => dispatch('remove')}
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
</div>

<style>
    .editor-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 1rem;
    }

    .editor-header-actions {
        display: flex;
        gap: 0.5rem;
    }

    .editor-crop-btn {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        background: #27272a;
        color: #e4e4e7;
        border: 1px solid #3f3f46;
        padding: 0.4rem 0.8rem;
        border-radius: 4px;
        font-size: 0.8rem;
        cursor: pointer;
        transition: all 0.2s ease;
    }

    .editor-crop-btn:hover {
        background: #3f3f46;
        border-color: #52525b;
    }

    .editor-crop-btn.active {
        background: #3b82f6;
        border-color: #60a5fa;
        color: white;
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
</style>
