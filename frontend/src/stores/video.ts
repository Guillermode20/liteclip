import { writable } from 'svelte/store';

interface VideoState {
    file: File | null;
    objectUrl: string | null;
}

function createVideoStore() {
    const { subscribe, set, update } = writable<VideoState>({
        file: null,
        objectUrl: null
    });

    return {
        subscribe,
        setFile: (file: File | null) => {
            update(state => {
                // Clean up previous object URL
                if (state.objectUrl) {
                    URL.revokeObjectURL(state.objectUrl);
                }
                
                return {
                    file,
                    objectUrl: file ? URL.createObjectURL(file) : null
                };
            });
        },
        reset: () => {
            update(state => {
                if (state.objectUrl) {
                    URL.revokeObjectURL(state.objectUrl);
                }
                return { file: null, objectUrl: null };
            });
        }
    };
}

export const videoStore = createVideoStore();
