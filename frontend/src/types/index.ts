export type StatusMessageType = 'processing' | 'success' | 'error';

export type CodecKey = 'fast' | 'quality';
export type ResolutionPreset = 'auto' | 'source' | '1080p' | '720p' | '480p' | '360p';

export interface VideoSegment {
    start: number;
    end: number;
}

export interface OutputMetadata {
    outputSizeBytes: number;
    outputSizeMb: number;
    compressionRatio: number;
    targetBitrateKbps: number;
    videoBitrateKbps: number;
    estimatedVideoBitrateKbps: number;
    scalePercent: number;
    codec: string;
    encoderName: string | null;
    encoderIsHardware: boolean;
    encodingTime: number;
    finalDuration: number;
    finalWidth: number;
    finalHeight: number;
}

export interface CodecDetail {
    helper: string;
    container: string;
}

export type CodecDetailsMap = Record<CodecKey, CodecDetail>;

export interface CompressionStatusResponse {
    status: 'queued' | 'processing' | 'completed' | 'failed' | 'cancelled';
    queuePosition?: number;
    progress?: number;
    estimatedSecondsRemaining?: number;
    codec?: string;
    encoderName?: string | null;
    encoderIsHardware?: boolean;
    outputFilename?: string;
    outputMimeType?: string;
    targetBitrateKbps?: number;
    videoBitrateKbps?: number;
    scalePercent?: number;
    encoder?: string;
    outputSizeBytes?: number;
    createdAt?: string;
    completedAt?: string;
    compressionSkipped?: boolean;
    message?: string;
}

export interface UpdateInfoPayload {
    currentVersion: string;
    latestVersion: string;
    updateAvailable: boolean;
    downloadUrl?: string | null;
    checkedAt?: string;
    releaseNotes?: string | null;
}

export interface UserSettingsPayload {
    defaultCodec: CodecKey;
    defaultResolution: ResolutionPreset;
    defaultMuteAudio: boolean;
    defaultTargetSizeMb: number;
    checkForUpdatesOnLaunch: boolean;
    appScale: number;
}

// Encoder detection removed: frontend no longer receives probe data

export type FfmpegStatusState = 'idle' | 'checking' | 'downloading' | 'ready' | 'error';

export interface FfmpegStatusResponse {
    state: FfmpegStatusState;
    progressPercent?: number;
    downloadedBytes?: number;
    totalBytes?: number;
    message?: string | null;
    errorMessage?: string | null;
    ready: boolean;
}

export interface EncoderInfo {
    name: string;
    description?: string | null;
    isHardware: boolean;
    isAvailable?: boolean | null;
    notes?: string | null;
}


