import type { CodecDetailsMap, OutputMetadata } from './types';

export const codecDetails: CodecDetailsMap = {
    fast: {
        helper: 'H.264 optimized for speed. Best for quick processing and wide device compatibility.',
        container: 'mp4'
    },
    quality: {
        helper: 'H.265 optimized for quality and file size. Better compression with good encoding speed.',
        container: 'mp4'
    },
    // NOTE: 'ultra' mode removed from frontend UI due to performance and compatibility concerns.
};

export function createDefaultOutputMetadata(): OutputMetadata {
    return {
        outputSizeBytes: 0,
        outputSizeMb: 0,
        compressionRatio: 0,
        targetBitrateKbps: 0,
        videoBitrateKbps: 0,
        estimatedVideoBitrateKbps: 0,
        scalePercent: 100,
        codec: 'quality',
        encoderName: null,
        encoderIsHardware: false,
        encodingTime: 0,
        finalDuration: 0,
        finalWidth: 0,
        finalHeight: 0
    };
}

