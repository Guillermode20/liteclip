import { writable } from 'svelte/store';
import type { UserSettingsPayload } from '../types';
import { getSettings, saveSettings as apiSaveSettings } from '../services/api';

const fallbackSettings: UserSettingsPayload = {
    defaultCodec: 'quality',
    defaultResolution: 'auto',
    defaultMuteAudio: false,
    defaultTargetSizeMb: 25,
    checkForUpdatesOnLaunch: true
};

function createSettingsStore() {
    const { subscribe, set, update } = writable<UserSettingsPayload>(fallbackSettings);

    return {
        subscribe,
        load: async () => {
            try {
                const settings = await getSettings();
                set(settings);
            } catch (error) {
                console.warn('Failed to load settings, using fallback', error);
                set(fallbackSettings);
            }
        },
        save: async (newSettings: UserSettingsPayload) => {
            try {
                const saved = await apiSaveSettings(newSettings);
                set(saved);
                return saved;
            } catch (error) {
                console.error('Failed to save settings', error);
                throw error;
            }
        },
        reset: () => set(fallbackSettings)
    };
}

export const settings = createSettingsStore();
