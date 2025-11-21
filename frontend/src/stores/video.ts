import { writable, derived, get } from 'svelte/store';
import type { VideoSegment } from '../types';
import { formatFileSize } from '../utils/format';
import { getEffectiveDuration, getEffectiveMaxSize } from '../utils/video';

interface VideoState {
    file: File | null;
    objectUrl: string | null;
    width: number | null;
    height: number | null;
    duration: number | null;
    originalSizeMb: number | null;
    segments: VideoSegment[];
    fileInfo: string;
    metadataVisible: boolean;
    metadataContent: string;
}

const initialState: VideoState = {
    file: null,
    objectUrl: null,
    width: null,
    height: null,
    duration: null,
    originalSizeMb: null,
    segments: [],
    fileInfo: '',
    metadataVisible: false,
    metadataContent: ''
};

function createVideoStore() {
    const { subscribe, set, update } = writable<VideoState>(initialState);

    return {
        subscribe,
        setFile: (file: File) => {
            const state = get({ subscribe });
            if (state.objectUrl) {
                URL.revokeObjectURL(state.objectUrl);
            }

            const objectUrl = URL.createObjectURL(file);
            const originalSizeMb = file.size / (1024 * 1024);
            const fileInfo = `Selected: ${file.name} (${formatFileSize(file.size)})`;

            set({
                ...initialState,
                file,
                objectUrl,
                originalSizeMb,
                fileInfo
            });

            // Metadata loading is handled separately via updateMetadata
        },
        updateMetadata: (width: number, height: number, duration: number) => {
            update(s => {
                if (!s.file) return s;
                
                const kbps = duration ? Math.round((s.file.size * 8) / duration / 1000) : null;
                const dimsText = `${width}×${height}`;
                const durationText = `${duration.toFixed(2)}s`;
                const bitrateText = kbps ? `${kbps} kbps (approx)` : 'Unknown';
                
                const metadataContent = `
                    <div><strong>file_size</strong>: ${formatFileSize(s.file.size)}</div>
                    <div><strong>type</strong>: ${s.file.type || 'unknown'}</div>
                    <div><strong>duration</strong>: ${durationText}</div>
                    <div><strong>resolution</strong>: ${dimsText}</div>
                    <div><strong>bitrate</strong>: ${bitrateText}</div>
                `;

                return {
                    ...s,
                    width,
                    height,
                    duration,
                    metadataVisible: true,
                    metadataContent
                };
            });
        },
        setSegments: (segments: VideoSegment[]) => {
            update(s => ({ ...s, segments }));
        },
        reset: () => {
            const state = get({ subscribe });
            if (state.objectUrl) {
                URL.revokeObjectURL(state.objectUrl);
            }
            set(initialState);
        }
    };
}

export const videoStore = createVideoStore();

export const effectiveMaxSize = derived(videoStore, $s => {
    return getEffectiveMaxSize($s.originalSizeMb, $s.duration, $s.segments);
});

export const effectiveDuration = derived(videoStore, $s => {
    return getEffectiveDuration($s.segments, $s.duration) ?? $s.duration;
});
