<script lang="ts">
    import { createEventDispatcher, onMount } from 'svelte';

    // All crop values are in percentages (0-100) relative to the video dimensions
    export let crop = { x: 0, y: 0, width: 100, height: 100 };
    export let isActive = false;

    const dispatch = createEventDispatcher<{
        change: { x: number; y: number; width: number; height: number };
    }>();

    let isDragging = false;
    let handle: string | null = null;
    let startX = 0;
    let startY = 0;
    let startCrop = { ...crop };
    let overlayElement: HTMLDivElement;

    function handleMouseDown(e: MouseEvent, handleName: string | null) {
        if (!isActive) return;
        e.preventDefault();
        e.stopPropagation();

        isDragging = true;
        handle = handleName;
        startX = e.clientX;
        startY = e.clientY;
        startCrop = { ...crop };

        window.addEventListener('mousemove', handleMouseMove);
        window.addEventListener('mouseup', handleMouseUp);
    }

    function handleMouseMove(e: MouseEvent) {
        if (!isDragging || !overlayElement) return;

        const rect = overlayElement.getBoundingClientRect();
        const dx = ((e.clientX - startX) / rect.width) * 100;
        const dy = ((e.clientY - startY) / rect.height) * 100;

        let { x, y, width, height } = { ...startCrop };

        if (!handle) {
            // Dragging entire crop box
            x = Math.max(0, Math.min(100 - width, startCrop.x + dx));
            y = Math.max(0, Math.min(100 - height, startCrop.y + dy));
        } else {
            if (handle.includes('top')) {
                const newY = Math.max(0, Math.min(startCrop.y + startCrop.height - 5, startCrop.y + dy));
                height = startCrop.y + startCrop.height - newY;
                y = newY;
            }
            if (handle.includes('bottom')) {
                height = Math.max(5, Math.min(100 - startCrop.y, startCrop.height + dy));
            }
            if (handle.includes('left')) {
                const newX = Math.max(0, Math.min(startCrop.x + startCrop.width - 5, startCrop.x + dx));
                width = startCrop.x + startCrop.width - newX;
                x = newX;
            }
            if (handle.includes('right')) {
                width = Math.max(5, Math.min(100 - startCrop.x, startCrop.width + dx));
            }
        }

        dispatch('change', { x, y, width, height });
    }

    function handleMouseUp() {
        isDragging = false;
        handle = null;
        window.removeEventListener('mousemove', handleMouseMove);
        window.removeEventListener('mouseup', handleMouseUp);
    }

    onMount(() => {
        return () => {
            window.removeEventListener('mousemove', handleMouseMove);
            window.removeEventListener('mouseup', handleMouseUp);
        };
    });
</script>

<div 
    class="crop-overlay-container" 
    class:active={isActive}
    bind:this={overlayElement}
>
    {#if isActive}
        <!-- Darkened background -->
        <div class="crop-backdrop top" style="height: {crop.y}%"></div>
        <div class="crop-backdrop bottom" style="top: {crop.y + crop.height}%; height: {100 - (crop.y + crop.height)}%"></div>
        <div class="crop-backdrop left" style="top: {crop.y}%; height: {crop.height}%; width: {crop.x}%"></div>
        <div class="crop-backdrop right" style="top: {crop.y}%; height: {crop.height}%; left: {crop.x + crop.width}%; width: {100 - (crop.x + crop.width)}%"></div>

        <!-- Crop selection box -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div 
            class="crop-box" 
            style="left: {crop.x}%; top: {crop.y}%; width: {crop.width}%; height: {crop.height}%"
            on:mousedown={(e) => handleMouseDown(e, null)}
        >
            <div class="crop-outline"></div>
            
            <!-- Grid lines -->
            <div class="grid-v grid-v-1"></div>
            <div class="grid-v grid-v-2"></div>
            <div class="grid-h grid-h-1"></div>
            <div class="grid-h grid-h-2"></div>

            <!-- Resizing handles -->
            <div class="handle top-left" on:mousedown={(e) => handleMouseDown(e, 'top-left')}></div>
            <div class="handle top-right" on:mousedown={(e) => handleMouseDown(e, 'top-right')}></div>
            <div class="handle bottom-left" on:mousedown={(e) => handleMouseDown(e, 'bottom-left')}></div>
            <div class="handle bottom-right" on:mousedown={(e) => handleMouseDown(e, 'bottom-right')}></div>
            <div class="handle top" on:mousedown={(e) => handleMouseDown(e, 'top')}></div>
            <div class="handle bottom" on:mousedown={(e) => handleMouseDown(e, 'bottom')}></div>
            <div class="handle left" on:mousedown={(e) => handleMouseDown(e, 'left')}></div>
            <div class="handle right" on:mousedown={(e) => handleMouseDown(e, 'right')}></div>
        </div>
    {/if}
</div>

<style>
    .crop-overlay-container {
        position: absolute;
        top: 0;
        left: 0;
        width: 100%;
        height: 100%;
        pointer-events: none;
        z-index: 10;
    }

    .crop-overlay-container.active {
        pointer-events: all;
    }

    .crop-backdrop {
        position: absolute;
        background: rgba(0, 0, 0, 0.5);
    }

    .crop-backdrop.top { top: 0; left: 0; width: 100%; }
    .crop-backdrop.bottom { left: 0; width: 100%; }
    .crop-backdrop.left { left: 0; }
    .crop-backdrop.right { }

    .crop-box {
        position: absolute;
        cursor: move;
        box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0); /* Fallback for older browsers if needed */
    }

    .crop-outline {
        position: absolute;
        top: 0;
        left: 0;
        right: 0;
        bottom: 0;
        border: 2px solid #3b82f6;
        box-shadow: 0 0 4px rgba(0, 0, 0, 0.5);
    }

    .grid-v, .grid-h {
        position: absolute;
        background: rgba(255, 255, 255, 0.2);
    }

    .grid-v { width: 1px; height: 100%; top: 0; }
    .grid-v-1 { left: 33.33%; }
    .grid-v-2 { left: 66.66%; }

    .grid-h { height: 1px; width: 100%; left: 0; }
    .grid-h-1 { top: 33.33%; }
    .grid-h-2 { top: 66.66%; }

    .handle {
        position: absolute;
        width: 12px;
        height: 12px;
        background: #fff;
        border: 2px solid #3b82f6;
        border-radius: 50%;
        z-index: 20;
    }

    .top-left { top: -6px; left: -6px; cursor: nwse-resize; }
    .top-right { top: -6px; right: -6px; cursor: nesw-resize; }
    .bottom-left { bottom: -6px; left: -6px; cursor: nesw-resize; }
    .bottom-right { bottom: -6px; right: -6px; cursor: nwse-resize; }
    
    .top { top: -6px; left: 50%; transform: translateX(-50%); cursor: ns-resize; width: 24px; border-radius: 4px; }
    .bottom { bottom: -6px; left: 50%; transform: translateX(-50%); cursor: ns-resize; width: 24px; border-radius: 4px; }
    .left { left: -6px; top: 50%; transform: translateY(-50%); cursor: ew-resize; height: 24px; border-radius: 4px; }
    .right { right: -6px; top: 50%; transform: translateY(-50%); cursor: ew-resize; height: 24px; border-radius: 4px; }
</style>
