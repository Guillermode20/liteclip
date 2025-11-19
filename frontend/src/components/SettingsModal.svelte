<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import type { CodecKey, ResolutionPreset, UserSettingsPayload } from '../lib/types';

    const dispatch = createEventDispatcher();

    export let open = false;
    export let settings: UserSettingsPayload | null = null;

    const defaultState: UserSettingsPayload = {
        defaultCodec: 'quality',
        defaultResolution: 'auto',
        defaultMuteAudio: false,
        defaultTargetSizeMb: 25,
        checkForUpdatesOnLaunch: true
    };

    let localSettings: UserSettingsPayload = { ...defaultState };

    $: if (open) {
        localSettings = { ...defaultState, ...settings };
    }

    function handleClose() {
        dispatch('close');
    }

    function handleSubmit(event: Event) {
        event.preventDefault();
        dispatch('save', { ...localSettings });
    }

    function handleCodecChange(event: Event) {
        localSettings = { ...localSettings, defaultCodec: (event.target as HTMLSelectElement).value as CodecKey };
    }

    function handleResolutionChange(event: Event) {
        localSettings = {
            ...localSettings,
            defaultResolution: (event.target as HTMLSelectElement).value as ResolutionPreset
        };
    }

    function handleTargetMbChange(event: Event) {
        const value = parseFloat((event.target as HTMLInputElement).value);
        if (!Number.isNaN(value)) {
            localSettings = { ...localSettings, defaultTargetSizeMb: Math.max(1, value) };
        }
    }

    function handleMuteToggle(event: Event) {
        localSettings = { ...localSettings, defaultMuteAudio: (event.target as HTMLInputElement).checked };
    }

    function handleUpdateToggle(event: Event) {
        localSettings = { ...localSettings, checkForUpdatesOnLaunch: (event.target as HTMLInputElement).checked };
    }
</script>

{#if open}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="modal-backdrop" on:click|self={handleClose}>
        <div class="settings-modal" role="dialog" aria-modal="true" aria-label="Settings">
            <form on:submit={handleSubmit}>
                <header class="modal-header">
                    <h2>// settings</h2>
                    <button type="button" class="icon-button" on:click={handleClose} aria-label="Close settings">
                        ✕
                    </button>
                </header>

                <div class="modal-body">
                    <div class="form-group">
                        <label for="defaultCodec">default codec</label>
                        <select id="defaultCodec" value={localSettings.defaultCodec} on:change={handleCodecChange}>
                            <option value="fast">fast (h.264)</option>
                            <option value="quality">quality (h.265)</option>
                        </select>
                    </div>

                    <div class="form-group">
                        <label for="defaultResolution">default resolution</label>
                        <select
                            id="defaultResolution"
                            value={localSettings.defaultResolution}
                            on:change={handleResolutionChange}
                        >
                            <option value="auto">auto (smart)</option>
                            <option value="source">original</option>
                            <option value="1080p">1080p</option>
                            <option value="720p">720p</option>
                            <option value="480p">480p</option>
                            <option value="360p">360p</option>
                        </select>
                    </div>

                    <div class="form-group">
                        <!-- svelte-ignore a11y_label_has_associated_control -->
                        <label>default compression target</label>
                        <input
                            type="range"
                            min="1"
                            max="100"
                            step="1"
                            value={localSettings.defaultTargetSizeMb}
                            on:input={handleTargetMbChange}
                        />
                        <div class="helper-text">{localSettings.defaultTargetSizeMb} MB</div>
                    </div>

                    <div class="form-group toggle">
                        <label>
                            <input type="checkbox" checked={localSettings.defaultMuteAudio} on:change={handleMuteToggle} />
                            mute audio by default
                        </label>
                    </div>

                    <div class="form-group toggle">
                        <label>
                            <input
                                type="checkbox"
                                checked={localSettings.checkForUpdatesOnLaunch}
                                on:change={handleUpdateToggle}
                            />
                            check for updates on launch
                        </label>
                    </div>
                </div>

                <footer class="modal-footer">
                    <button type="button" class="secondary" on:click={handleClose}>cancel</button>
                    <button type="submit" class="primary">save</button>
                </footer>
            </form>
        </div>
    </div>
{/if}

<style>
    .modal-backdrop {
        position: fixed;
        inset: 0;
        background: rgba(0, 0, 0, 0.6);
        display: flex;
        align-items: center;
        justify-content: center;
        z-index: 200;
        padding: 24px;
    }

    .settings-modal {
        width: 100%;
        max-width: 480px;
        background: #0f0f14;
        border: 1px solid #27272a;
        border-radius: 6px;
        box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
        animation: fadeIn 0.2s ease-out;
    }

    form {
        display: flex;
        flex-direction: column;
        gap: 16px;
        padding: 16px;
    }

    .modal-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
    }

    .modal-header h2 {
        margin: 0;
        font-size: 1rem;
        text-transform: lowercase;
        color: #fafafa;
    }

    .icon-button {
        border: 1px solid #27272a;
        background: transparent;
        color: #fafafa;
        border-radius: 4px;
        width: 28px;
        height: 28px;
        cursor: pointer;
        font-size: 0.9rem;
    }

    .modal-body {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .form-group {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .form-group label {
        font-size: 0.85rem;
        color: #a1a1aa;
        text-transform: lowercase;
    }

    select,
    input[type='range'] {
        width: 100%;
    }

    select {
        padding: 10px 12px;
        border-radius: 4px;
        border: 1px solid #27272a;
        background: rgba(24, 24, 27, 0.8);
        color: #fafafa;
    }

    input[type='range'] {
        accent-color: #22d3ee;
    }

    .helper-text {
        font-size: 0.75rem;
        color: #71717a;
    }

    .form-group.toggle label {
        display: inline-flex;
        align-items: center;
        gap: 8px;
        color: #fafafa;
        font-size: 0.85rem;
        text-transform: lowercase;
    }

    .modal-footer {
        display: flex;
        justify-content: flex-end;
        gap: 12px;
    }

    .modal-footer button {
        padding: 8px 14px;
        border-radius: 4px;
        border: 1px solid #27272a;
        cursor: pointer;
        text-transform: lowercase;
    }

    .modal-footer .secondary {
        background: transparent;
        color: #a1a1aa;
    }

    .modal-footer .primary {
        background: #22d3ee;
        color: #0f0f14;
        border-color: #22d3ee;
        font-weight: 600;
    }

    @keyframes fadeIn {
        from {
            opacity: 0;
            transform: translateY(10px);
        }
        to {
            opacity: 1;
            transform: translateY(0);
        }
    }
</style>
