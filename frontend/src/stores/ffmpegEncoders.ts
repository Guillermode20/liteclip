import { writable } from 'svelte/store';

interface Encoder {
    name: string;
    type: 'video' | 'audio';
    isHardware: boolean;
    codec: string;
}

interface FfmpegEncodersState {
    encoders: Encoder[];
    loading: boolean;
    error: string | null;
}

function createFfmpegEncodersStore() {
    const { subscribe, set, update } = writable<FfmpegEncodersState>({
        encoders: [],
        loading: false,
        error: null
    });

    async function fetchEncoders(forceRefresh = false) {
        update(state => ({ ...state, loading: true, error: null }));
        
        try {
            const url = forceRefresh ? '/api/encoders?refresh=true' : '/api/encoders';
            const response = await fetch(url);
            if (!response.ok) {
                throw new Error(`Failed to load encoders: ${response.status}`);
            }
            const data = await response.json();
            set({
                encoders: data.encoders || [],
                loading: false,
                error: null
            });
        } catch (error) {
            set({
                encoders: [],
                loading: false,
                error: (error as Error).message
            });
        }
    }

    return {
        subscribe,
        load: () => fetchEncoders(false),
        refresh: (forceRefresh = false) => fetchEncoders(forceRefresh),
        reset: () => {
            set({ encoders: [], loading: false, error: null });
        }
    };
}

export const ffmpegEncodersStore = createFfmpegEncodersStore();
