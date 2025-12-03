import type { VideoSegment } from '../types';

export interface SegmentWithId extends VideoSegment {
    id: string;
}

let nextSegmentId = 1;

/**
 * Creates a new segment range with a unique ID.
 */
export function createSegmentRange(start: number, end: number): SegmentWithId {
    return {
        start,
        end,
        id: `seg-${nextSegmentId++}`
    };
}

/**
 * Resets the segment ID counter.
 */
export function resetSegmentCounter(): void {
    nextSegmentId = 1;
}

/**
 * Clamps a time value to valid range.
 */
export function clampTime(value: number, duration: number): number {
    if (!Number.isFinite(value)) return 0;
    if (value < 0) return 0;
    if (value > duration) return duration;
    return value;
}

/**
 * Sanitizes saved segments to ensure they're valid.
 */
export function sanitizeSavedSegments(
    savedSegments: VideoSegment[] | null | undefined,
    duration: number
): VideoSegment[] {
    if (!savedSegments || savedSegments.length === 0 || !Number.isFinite(duration) || duration <= 0) {
        return [];
    }

    return savedSegments
        .map(seg => ({
            start: clampTime(seg.start, duration),
            end: clampTime(seg.end, duration)
        }))
        .filter(seg => seg.end > seg.start)
        .sort((a, b) => a.start - b.start);
}

/**
 * Initializes segments from saved state or creates a default full-video segment.
 */
export function initializeSegments(
    savedSegments: VideoSegment[] | null | undefined,
    duration: number
): SegmentWithId[] {
    resetSegmentCounter();
    
    const restored = sanitizeSavedSegments(savedSegments, duration);
    if (restored.length > 0) {
        return restored.map(seg => createSegmentRange(seg.start, seg.end));
    }

    return [createSegmentRange(0, duration)];
}

/**
 * Cuts a segment at the specified time, creating two new segments.
 */
export function cutSegmentAtTime(
    segments: SegmentWithId[],
    cutTime: number,
    duration: number
): SegmentWithId[] | null {
    if (cutTime <= 0 || cutTime >= duration) return null;

    const segmentIndex = segments.findIndex(seg =>
        cutTime > seg.start && cutTime < seg.end
    );

    if (segmentIndex === -1) return null;

    const segment = segments[segmentIndex];
    const leftSegment = createSegmentRange(segment.start, cutTime);
    const rightSegment = createSegmentRange(cutTime, segment.end);

    return [
        ...segments.slice(0, segmentIndex),
        leftSegment,
        rightSegment,
        ...segments.slice(segmentIndex + 1)
    ];
}

/**
 * Removes a segment by ID.
 */
export function deleteSegment(segments: SegmentWithId[], segmentId: string): SegmentWithId[] {
    return segments.filter(s => s.id !== segmentId);
}

/**
 * Converts segments with IDs to plain video segments for external use.
 */
export function toVideoSegments(segments: SegmentWithId[]): VideoSegment[] {
    return segments
        .sort((a, b) => a.start - b.start)
        .map(s => ({ start: s.start, end: s.end }));
}

/**
 * Calculates total duration of all segments.
 */
export function getTotalDuration(segments: SegmentWithId[]): number {
    return segments.reduce((sum, seg) => sum + (seg.end - seg.start), 0);
}
