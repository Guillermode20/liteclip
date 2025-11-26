/**
 * Video loading and metadata extraction service.
 * Provides a clean, promise-based API for loading videos and extracting metadata.
 */

export interface VideoMetadata {
    width: number;
    height: number;
    duration: number;
    aspectRatio: number;
}

export interface VideoLoadResult {
    objectUrl: string;
    metadata: VideoMetadata;
}

export interface VideoLoadError {
    type: 'load' | 'metadata' | 'timeout' | 'aborted';
    message: string;
}

const DEFAULT_TIMEOUT_MS = 30000;

/**
 * Extracts metadata from a video element that has loaded its metadata.
 */
function extractMetadata(video: HTMLVideoElement): VideoMetadata | null {
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
    // Keep element off-screen but in DOM for reliable loading
    video.style.cssText = 'position:absolute;width:1px;height:1px;opacity:0;pointer-events:none;';
    return video;
}

/**
 * Loads a video file and extracts its metadata.
 * Returns a promise that resolves with the object URL and metadata,
 * or rejects with a VideoLoadError.
 * 
 * @param file - The video file to load
 * @param signal - Optional AbortSignal to cancel the operation
 * @param timeoutMs - Timeout in milliseconds (default: 30000)
 */
export function loadVideoFile(
    file: File,
    signal?: AbortSignal,
    timeoutMs: number = DEFAULT_TIMEOUT_MS
): Promise<VideoLoadResult> {
    return new Promise((resolve, reject) => {
        if (signal?.aborted) {
            reject({ type: 'aborted', message: 'Operation was aborted' } as VideoLoadError);
            return;
        }

        const objectUrl = URL.createObjectURL(file);
        const video = createProbeVideo();
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
            // Remove from DOM if it was added
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
            URL.revokeObjectURL(objectUrl);
            reject(error);
        };

        const succeed = (result: VideoLoadResult) => {
            if (settled) return;
            settled = true;
            cleanup();
            resolve(result);
        };

        const onMetadataLoaded = () => {
            const metadata = extractMetadata(video);
            if (!metadata) {
                fail({
                    type: 'metadata',
                    message: 'Failed to extract valid metadata from video'
                });
                return;
            }
            succeed({ objectUrl, metadata });
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

        // Start loading - add to DOM first for more reliable loading
        document.body.appendChild(video);
        video.src = objectUrl;
    });
}

/**
 * Extracts metadata from an already-loaded video element.
 * Useful when you have a video element that's already displaying content.
 */
export function getMetadataFromElement(video: HTMLVideoElement): VideoMetadata | null {
    if (video.readyState < HTMLMediaElement.HAVE_METADATA) {
        return null;
    }
    return extractMetadata(video);
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
            const metadata = extractMetadata(video);
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
            const metadata = extractMetadata(video);
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
