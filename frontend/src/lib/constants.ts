import type { CodecDetailsMap, OutputMetadata, UserSettingsPayload } from '../types';

export const codecDetails: CodecDetailsMap = {
    fast: {
        helper: 'H.264 optimized for speed. Best for quick processing and wide device compatibility.',
        container: 'mp4'
    },
    quality: {
        helper: 'H.265 optimized for quality and file size. Better compression with good encoding speed.',
        container: 'mp4'
    },
    ultra: {
        helper: 'Extreme H.265 software encoding. Slowest but achieves highest possible quality for the target size.',
        container: 'mp4'
    },
};

export const FALLBACK_SETTINGS: UserSettingsPayload = {
    defaultCodec: 'quality',
    defaultResolution: 'auto',
    defaultMuteAudio: false,
    defaultTargetSizeMb: 25,
    checkForUpdatesOnLaunch: true,
    appScale: 1.0
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

