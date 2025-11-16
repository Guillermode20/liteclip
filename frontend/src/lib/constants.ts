import type { CodecDetailsMap, OutputMetadata } from './types';

export const codecDetails: CodecDetailsMap = {
    h264: {
        helper: 'Best compatibility across browsers and devices.',
        container: 'mp4'
    },
    h265: {
        helper: 'Higher efficiency than H.264 but slower to encode. Limited support on older devices.',
        container: 'mp4'
    },
    vp9: {
        helper: 'Great for modern browsers. Outputs WebM files optimized for streaming.',
        container: 'webm'
    },
    av1: {
        helper: 'Smallest files but slowest encode. Requires very recent hardware/software.',
        container: 'webm'
    }
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
        codec: 'h264',
        encoderName: null,
        encoderIsHardware: false,
        encodingTime: 0,
        finalDuration: 0,
        finalWidth: 0,
        finalHeight: 0
    };
}

