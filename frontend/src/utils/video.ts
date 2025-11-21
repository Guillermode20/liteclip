import type { VideoSegment } from '../types';

export function getEffectiveDuration(segments: VideoSegment[], sourceDuration: number | null): number | null {
    if (segments.length > 0) {
        return segments.reduce((sum, seg) => sum + (seg.end - seg.start), 0);
    }
    return sourceDuration;
}

export function getEffectiveMaxSize(
    originalSizeMb: number | null,
    sourceDuration: number | null,
    segments: VideoSegment[]
): number {
    if (!originalSizeMb || !Number.isFinite(originalSizeMb)) {
        return 0;
    }
    if (!sourceDuration || sourceDuration <= 0) {
        return originalSizeMb;
    }

    const effectiveDuration = getEffectiveDuration(segments, sourceDuration);
    if (!effectiveDuration || effectiveDuration === sourceDuration) {
        return originalSizeMb;
    }

    const durationRatio = effectiveDuration / sourceDuration;
    return originalSizeMb * durationRatio;
}

export function calculateOptimalResolution(
    targetSizeMb: number,
    durationSec: number,
    width: number,
    height: number
): number {
    if (
        !Number.isFinite(targetSizeMb) ||
        !Number.isFinite(durationSec) ||
        !Number.isFinite(width) ||
        !Number.isFinite(height) ||
        targetSizeMb <= 0 ||
        durationSec <= 0 ||
        width <= 0 ||
        height <= 0
    ) {
        return 100;
    }

    const targetBitsTotal = targetSizeMb * 1024 * 1024 * 8 * 0.97;
    const targetBitrateKbps = targetBitsTotal / durationSec / 1000;
    const videoBitrateKbps = targetBitrateKbps - 128;
    const pixels = width * height;
    const bitsPerPixel = (videoBitrateKbps * 1000) / pixels / 30;

    if (bitsPerPixel >= 0.1) {
        return 100;
    }

    const targetBpp = 0.1;
    const scaleFactor = Math.sqrt(bitsPerPixel / targetBpp);
    let scalePercent = Math.min(100, Math.round(scaleFactor * 100));

    const minHeight = 480;
    // Only enforce minimum output height if the source is at least that tall.
    if (height >= minHeight) {
        const heightScalePercent = Math.ceil((minHeight / height) * 100);
        scalePercent = Math.max(scalePercent, heightScalePercent);
    }

    // Do not allow upscaling above 100, and respect a sensible floor of 25%.
    scalePercent = Math.min(100, Math.max(25, scalePercent));
    return scalePercent;
}

