export type StatusMessageType = 'processing' | 'success' | 'error';

export type CodecKey = 'h264' | 'h265' | 'vp9' | 'av1';

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

