import type { ResolutionPreset } from '../types';

/**
 * Parses a resolution preset string to its target height in pixels.
 */
export function parseResolutionHeight(preset: ResolutionPreset): number | null {
    switch (preset) {
        case '1080p':
            return 1080;
        case '720p':
            return 720;
        case '480p':
            return 480;
        case '360p':
            return 360;
        default:
            return null;
    }
}

/**
 * Calculates the forced scale percentage based on resolution preset and source height.
 */
export function getForcedScalePercent(
    sourceVideoHeight: number | null,
    resolutionPreset: ResolutionPreset
): number | null {
    if (!sourceVideoHeight || resolutionPreset === 'auto') {
        return null;
    }

    if (resolutionPreset === 'source') {
        return 100;
    }

    const targetHeight = parseResolutionHeight(resolutionPreset);
    if (!targetHeight || targetHeight <= 0) {
        return null;
    }

    if (sourceVideoHeight <= targetHeight) {
        return 100;
    }

    const percent = Math.round((targetHeight / sourceVideoHeight) * 100);
    return Math.max(10, Math.min(100, percent));
}

/**
 * Clamps a percentage value to valid range (1-100).
 */
export function clampPercentValue(value: number | null | undefined): number {
    if (typeof value !== 'number' || !Number.isFinite(value)) {
        return 100;
    }
    return Math.min(100, Math.max(1, value));
}

/**
 * Formats resolution for display.
 */
export function formatResolution(width: number, height: number, sourceWidth?: number): string {
    if (width <= 0 || height <= 0) {
        return '--';
    }
    
    let label = `${width}×${height}`;
    if (sourceWidth && sourceWidth > 0) {
        const percent = Math.round((width / sourceWidth) * 100);
        label += ` (${percent}%)`;
    }
    return label;
}
