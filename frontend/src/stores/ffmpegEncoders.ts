import { writable } from 'svelte/store';
import type { EncoderInfo } from '../types';
import { getFfmpegEncoders } from '../services/api';

interface EncodersStoreState {
    encoders: EncoderInfo[];
    loading: boolean;
    error: string | null;
    lastUpdated: number | null;
}

const TTL_MS = 5 * 60 * 1000; // 5 minutes

function createEncodersStore() {
    const { subscribe, set, update } = writable<EncodersStoreState>({
        encoders: [],
        loading: false,
        error: null,
        lastUpdated: null
    });

    async function load(force = false, verify = false) {
        let shouldLoad = force;
        update(state => {
            if (state.lastUpdated === null) shouldLoad = true;
            if (!force && state.lastUpdated) {
                const age = Date.now() - state.lastUpdated;
                if (age > TTL_MS) shouldLoad = true;
            }
            return { ...state, loading: shouldLoad };
        });

        if (!shouldLoad) return;

        try {
            const enc = await getFfmpegEncoders(verify);
            set({ encoders: enc, loading: false, error: null, lastUpdated: Date.now() });
        } catch (err) {
            set({ encoders: [], loading: false, error: (err as Error).message || 'Failed to load encoders', lastUpdated: Date.now() });
        }
    }

    function refresh(verify = false) {
        return load(true, verify);
    }

    return { subscribe, load, refresh };
}

export const ffmpegEncodersStore = createEncodersStore();
