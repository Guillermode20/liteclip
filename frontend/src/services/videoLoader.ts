/**
 * Video loading and metadata extraction service.
 * Uses ffprobe backend for reliable metadata extraction with browser fallback.
 */

export interface VideoMetadata {
    width: number;
    height: number;
    duration: number;
    aspectRatio: number;
    codec?: string;
    frameRate?: number;
    bitrate?: number;
    pixelFormat?: string;
    hasAudio?: boolean;
    audioCodec?: string;
    audioChannels?: number;
    audioSampleRate?: number;
}

export interface VideoLoadResult {
    objectUrl: string;
    metadata: VideoMetadata;
    source: 'backend' | 'browser';
}

export interface VideoLoadError {
    type: 'load' | 'metadata' | 'timeout' | 'aborted' | 'network';
    message: string;
}

const DEFAULT_TIMEOUT_MS = 30000;
const BACKEND_TIMEOUT_MS = 30000;
const BROWSER_TIMEOUT_MS = 5000; // Browser probe should be fast - file is local

// API response type from backend
interface BackendMetadataResponse {
    width: number;
    height: number;
    duration: number;
    aspectRatio: number;
    codec?: string;
    frameRate?: number;
    bitrate?: number;
    pixelFormat?: string;
    hasAudio?: boolean;
    audioCodec?: string;
    audioChannels?: number;
    audioSampleRate?: number;
}

/**
 * Probes video metadata using the backend ffprobe service.
 * This is more reliable than browser-based extraction for all video formats.
 */
async function probeWithBackend(
    file: File,
    signal?: AbortSignal
): Promise<VideoMetadata | null> {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), BACKEND_TIMEOUT_MS);
    
    // Link external signal if provided
    if (signal) {
        signal.addEventListener('abort', () => controller.abort(), { once: true });
    }

    try {
        const formData = new FormData();
        formData.append('file', file);

        const response = await fetch('/api/probe-metadata', {
            method: 'POST',
            body: formData,
            signal: controller.signal
        });

        clearTimeout(timeoutId);

        if (!response.ok) {
            if (import.meta.env.DEV) console.warn(`Backend probe failed with status ${response.status}`);
            return null;
        }

        const data: BackendMetadataResponse = await response.json();
        
        // Validate response
        if (!data.width || !data.height || !data.duration || 
            data.width <= 0 || data.height <= 0 || data.duration <= 0) {
            if (import.meta.env.DEV) console.warn('Backend returned invalid metadata:', data);
            return null;
        }

        return {
            width: data.width,
            height: data.height,
            duration: data.duration,
            aspectRatio: data.aspectRatio || data.width / data.height,
            codec: data.codec,
            frameRate: data.frameRate,
            bitrate: data.bitrate,
            pixelFormat: data.pixelFormat,
            hasAudio: data.hasAudio,
            audioCodec: data.audioCodec,
            audioChannels: data.audioChannels,
            audioSampleRate: data.audioSampleRate
        };
    } catch (err) {
        clearTimeout(timeoutId);
        if (import.meta.env.DEV) {
            if (err instanceof Error && err.name === 'AbortError') {
                console.warn('Backend probe aborted or timed out');
            } else {
                console.warn('Backend probe error:', err);
            }
        }
        return null;
    }
}

/**
 * Extracts metadata from a video element that has loaded its metadata.
 */
function extractBrowserMetadata(video: HTMLVideoElement): VideoMetadata | null {
    const duration = video.duration;
    const width = video.videoWidth;
    const height = video.videoHeight;

    if (!Number.isFinite(duration) || duration <= 0) {
        return null;
    }

    if (!width || !height || width <= 0 || height <= 0) {
        return null;
    }

    return {
        width,
        height,
        duration,
        aspectRatio: width / height
    };
}

/**
 * Creates a hidden video element for metadata extraction.
 */
function createProbeVideo(): HTMLVideoElement {
    const video = document.createElement('video');
    video.preload = 'metadata';
    video.muted = true;
    video.playsInline = true;
    video.style.cssText = 'position:absolute;width:1px;height:1px;opacity:0;pointer-events:none;';
    return video;
}

/**
 * Probes video metadata using the browser's HTML5 video element.
 * Fallback when backend is unavailable.
 */
function probeWithBrowser(
    objectUrl: string,
    signal?: AbortSignal,
    timeoutMs: number = BROWSER_TIMEOUT_MS
): Promise<VideoMetadata> {
    return new Promise((resolve, reject) => {
        if (signal?.aborted) {
            reject({ type: 'aborted', message: 'Operation was aborted' } as VideoLoadError);
            return;
        }

        const video = createProbeVideo();
        let timeoutId: ReturnType<typeof setTimeout> | null = null;
        let settled = false;
        let retryCount = 0;
        const maxRetries = 2;

        const cleanup = () => {
            if (timeoutId) {
                clearTimeout(timeoutId);
                timeoutId = null;
            }
            video.removeEventListener('loadedmetadata', onMetadataLoaded);
            video.removeEventListener('loadeddata', onLoadedData);
            video.removeEventListener('canplay', onCanPlay);
            video.removeEventListener('error', onError);
            signal?.removeEventListener('abort', onAbort);
            if (video.parentNode) {
                video.parentNode.removeChild(video);
            }
            video.src = '';
            video.load();
        };

        const fail = (error: VideoLoadError) => {
            if (settled) return;
            settled = true;
            cleanup();
            reject(error);
        };

        const succeed = (metadata: VideoMetadata) => {
            if (settled) return;
            settled = true;
            cleanup();
            resolve(metadata);
        };

        const tryExtract = (): boolean => {
            const metadata = extractBrowserMetadata(video);
            if (metadata) {
                succeed(metadata);
                return true;
            }
            return false;
        };

        const onMetadataLoaded = () => {
            // Sometimes loadedmetadata fires before dimensions are ready
            // Wait a tick and try, or wait for loadeddata/canplay
            setTimeout(() => {
                if (!tryExtract() && retryCount < maxRetries) {
                    retryCount++;
                    // Dimensions not ready yet, wait for more data
                }
            }, 50);
        };

        const onLoadedData = () => {
            tryExtract();
        };

        const onCanPlay = () => {
            if (!tryExtract()) {
                fail({
                    type: 'metadata',
                    message: 'Failed to extract valid metadata from video'
                });
            }
        };

        const onError = () => {
            const mediaError = video.error;
            let message = 'Failed to load video';
            if (mediaError) {
                switch (mediaError.code) {
                    case MediaError.MEDIA_ERR_ABORTED:
                        message = 'Video loading was aborted';
                        break;
                    case MediaError.MEDIA_ERR_NETWORK:
                        message = 'Network error while loading video';
                        break;
                    case MediaError.MEDIA_ERR_DECODE:
                        message = 'Video format is not supported or file is corrupted';
                        break;
                    case MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED:
                        message = 'Video format is not supported by your browser';
                        break;
                }
            }
            fail({ type: 'load', message });
        };

        const onAbort = () => {
            fail({ type: 'aborted', message: 'Operation was aborted' });
        };

        const onTimeout = () => {
            // Last attempt before failing
            if (!tryExtract()) {
                fail({
                    type: 'timeout',
                    message: `Browser metadata extraction timed out after ${timeoutMs}ms`
                });
            }
        };

        // Set up event listeners - listen to multiple events for reliability
        video.addEventListener('loadedmetadata', onMetadataLoaded, { once: true });
        video.addEventListener('loadeddata', onLoadedData, { once: true });
        video.addEventListener('canplay', onCanPlay, { once: true });
        video.addEventListener('error', onError, { once: true });
        signal?.addEventListener('abort', onAbort, { once: true });

        timeoutId = setTimeout(onTimeout, timeoutMs);

        // Add to DOM and start loading
        document.body.appendChild(video);
        video.src = objectUrl;
    });
}

/**
 * Loads a video file and extracts its metadata.
 * 
 * Strategy (optimized for speed):
 * 1. Try browser first (instant - no upload needed)
 * 2. Fall back to backend ffprobe only if browser fails (e.g., unsupported codec)
 * 
 * @param file - The video file to load
 * @param signal - Optional AbortSignal to cancel the operation
 * @param timeoutMs - Total timeout in milliseconds (default: 60000)
 */
export async function loadVideoFile(
    file: File,
    signal?: AbortSignal,
    timeoutMs: number = DEFAULT_TIMEOUT_MS
): Promise<VideoLoadResult> {
    if (signal?.aborted) {
        throw { type: 'aborted', message: 'Operation was aborted' } as VideoLoadError;
    }

    // Create object URL for browser playback (always needed for preview)
    const objectUrl = URL.createObjectURL(file);

    try {
        // Strategy 1: Try browser first (instant, no upload required)
        if (import.meta.env.DEV) console.log('Attempting browser metadata probe...');
        try {
            const browserMetadata = await probeWithBrowser(objectUrl, signal, BROWSER_TIMEOUT_MS);
            if (import.meta.env.DEV) console.log('Browser probe successful:', browserMetadata);
            return {
                objectUrl,
                metadata: browserMetadata,
                source: 'browser'
            };
        } catch (browserError) {
            if (import.meta.env.DEV) console.log('Browser probe failed:', browserError);
            // Continue to backend fallback
        }

        // Strategy 2: Fall back to backend ffprobe (handles all formats but requires upload)
        if (import.meta.env.DEV) console.log('Falling back to backend ffprobe...');
        const backendMetadata = await probeWithBackend(file, signal);
        
        if (backendMetadata) {
            if (import.meta.env.DEV) console.log('Backend probe successful:', backendMetadata);
            return {
                objectUrl,
                metadata: backendMetadata,
                source: 'backend'
            };
        }

        // Both failed
        throw { 
            type: 'metadata', 
            message: 'Failed to extract video metadata. The file may be corrupted or in an unsupported format.' 
        } as VideoLoadError;
    } catch (error) {
        // Clean up object URL on failure
        URL.revokeObjectURL(objectUrl);
        throw error;
    }
}

/**
 * Extracts metadata from an already-loaded video element.
 * Useful when you have a video element that's already displaying content.
 */
export function getMetadataFromElement(video: HTMLVideoElement): VideoMetadata | null {
    if (video.readyState < HTMLMediaElement.HAVE_METADATA) {
        return null;
    }
    return extractBrowserMetadata(video);
}

/**
 * Waits for a video element to load its metadata.
 * Returns a promise that resolves when metadata is available.
 */
export function waitForMetadata(
    video: HTMLVideoElement,
    signal?: AbortSignal,
    timeoutMs: number = DEFAULT_TIMEOUT_MS
): Promise<VideoMetadata> {
    return new Promise((resolve, reject) => {
        if (signal?.aborted) {
            reject({ type: 'aborted', message: 'Operation was aborted' } as VideoLoadError);
            return;
        }

        // If metadata is already loaded, extract and return immediately
        if (video.readyState >= HTMLMediaElement.HAVE_METADATA) {
            const metadata = extractBrowserMetadata(video);
            if (metadata) {
                resolve(metadata);
                return;
            }
        }

        let timeoutId: ReturnType<typeof setTimeout> | null = null;
        let settled = false;

        const cleanup = () => {
            if (timeoutId) {
                clearTimeout(timeoutId);
                timeoutId = null;
            }
            video.removeEventListener('loadedmetadata', onMetadataLoaded);
            video.removeEventListener('error', onError);
            signal?.removeEventListener('abort', onAbort);
        };

        const fail = (error: VideoLoadError) => {
            if (settled) return;
            settled = true;
            cleanup();
            reject(error);
        };

        const succeed = (metadata: VideoMetadata) => {
            if (settled) return;
            settled = true;
            cleanup();
            resolve(metadata);
        };

        const onMetadataLoaded = () => {
            const metadata = extractBrowserMetadata(video);
            if (!metadata) {
                fail({
                    type: 'metadata',
                    message: 'Failed to extract valid metadata from video'
                });
                return;
            }
            succeed(metadata);
        };

        const onError = () => {
            const mediaError = video.error;
            let message = 'Failed to load video';
            if (mediaError) {
                switch (mediaError.code) {
                    case MediaError.MEDIA_ERR_ABORTED:
                        message = 'Video loading was aborted';
                        break;
                    case MediaError.MEDIA_ERR_NETWORK:
                        message = 'Network error while loading video';
                        break;
                    case MediaError.MEDIA_ERR_DECODE:
                        message = 'Video format is not supported or file is corrupted';
                        break;
                    case MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED:
                        message = 'Video format is not supported by your browser';
                        break;
                }
            }
            fail({ type: 'load', message });
        };

        const onAbort = () => {
            fail({ type: 'aborted', message: 'Operation was aborted' });
        };

        const onTimeout = () => {
            fail({
                type: 'timeout',
                message: `Video metadata extraction timed out after ${timeoutMs}ms`
            });
        };

        // Set up event listeners
        video.addEventListener('loadedmetadata', onMetadataLoaded, { once: true });
        video.addEventListener('error', onError, { once: true });
        signal?.addEventListener('abort', onAbort, { once: true });

        // Set up timeout
        timeoutId = setTimeout(onTimeout, timeoutMs);
    });
}

/**
 * Validates that a file is a supported video type.
 */
export function isVideoFile(file: File): boolean {
    return file.type.startsWith('video/');
}

/**
 * Gets a human-readable error message from a VideoLoadError.
 */
export function getErrorMessage(error: VideoLoadError): string {
    return error.message;
}
