/**
 * Video player utilities for timeline scrubbing and playback control.
 */

const VIDEO_UPDATE_INTERVAL_MS = 100;

export interface DragState {
    isDragging: boolean;
    wasPlayingBeforeDrag: boolean;
    visualScrubTime: number;
    lastVideoUpdateTime: number;
}

export function createDragState(): DragState {
    return {
        isDragging: false,
        wasPlayingBeforeDrag: false,
        visualScrubTime: 0,
        lastVideoUpdateTime: 0
    };
}

/**
 * Calculates the time position from a mouse event on the timeline.
 */
export function getTimeFromMouseEvent(
    event: MouseEvent,
    container: HTMLElement,
    duration: number
): number {
    const rect = container.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const time = (x / rect.width) * duration;
    return Math.max(0, Math.min(time, duration));
}

/**
 * Determines if enough time has passed to perform a video seek.
 */
export function shouldPerformSeek(lastUpdateTime: number): boolean {
    return performance.now() - lastUpdateTime >= VIDEO_UPDATE_INTERVAL_MS;
}

/**
 * Formats time in M:SS.T format.
 */
export function formatTime(seconds: number): string {
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    const ms = Math.floor((seconds % 1) * 10);
    return `${mins}:${secs.toString().padStart(2, '0')}.${ms}`;
}

/**
 * Formats time in M:SS format for markers.
 */
export function formatMarkerTime(seconds: number): string {
    if (!Number.isFinite(seconds) || seconds < 0) {
        return '0:00';
    }
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${mins}:${secs.toString().padStart(2, '0')}`;
}

/**
 * Cleans up resources (object URLs, animation frames, timeouts).
 */
export interface CleanupState {
    objectUrl: string | null;
    animationFrameId: number | null;
    timeUpdateRafId: number | null;
    videoSeekTimeout: ReturnType<typeof setTimeout> | null;
}

export function cleanupResources(state: CleanupState): CleanupState {
    if (state.objectUrl) {
        URL.revokeObjectURL(state.objectUrl);
    }
    if (state.animationFrameId) {
        cancelAnimationFrame(state.animationFrameId);
    }
    if (state.timeUpdateRafId) {
        cancelAnimationFrame(state.timeUpdateRafId);
    }
    if (state.videoSeekTimeout) {
        clearTimeout(state.videoSeekTimeout);
    }
    
    return {
        objectUrl: null,
        animationFrameId: null,
        timeUpdateRafId: null,
        videoSeekTimeout: null
    };
}
