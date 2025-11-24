import { writable } from 'svelte/store';
import type { UserSettingsPayload } from '../types';
import { getSettings, saveSettings as apiSaveSettings } from '../services/api';
import { FALLBACK_SETTINGS } from '../lib/constants';

function createSettingsStore() {
    const { subscribe, set, update } = writable<UserSettingsPayload>(FALLBACK_SETTINGS);

    return {
        subscribe,
        load: async () => {
            try {
                const settings = await getSettings();
                set(settings);
            } catch (error) {
                console.warn('Failed to load settings, using fallback', error);
                set(FALLBACK_SETTINGS);
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
        reset: () => set(FALLBACK_SETTINGS)
    };
}

export const settings = createSettingsStore();
