<script lang="ts">
    import { createEventDispatcher, onDestroy } from 'svelte';
    import type { CodecKey, ResolutionPreset, UserSettingsPayload } from '../types';

    const dispatch = createEventDispatcher();
    import { ffmpegEncodersStore } from '../stores/ffmpegEncoders';

    export let open = false;
    export let settings: UserSettingsPayload | null = null;

    type SettingsSection = 'compression' | 'appearance' | 'system';
    let activeSection: SettingsSection = 'compression';

    const sections: { id: SettingsSection; label: string; icon: string }[] = [
        { id: 'compression', label: 'Compression', icon: '⚙' },
        { id: 'appearance', label: 'Appearance', icon: '🎨' },
        { id: 'system', label: 'System', icon: '💻' }
    ];

    const defaultState: UserSettingsPayload = {
        defaultCodec: 'quality',
        defaultResolution: 'auto',
        defaultMuteAudio: false,
        defaultTargetSizeMb: 25,
        checkForUpdatesOnLaunch: true,
        appScale: 1.0
    };

    let localSettings: UserSettingsPayload = { ...defaultState };

    $: if (open) {
        localSettings = { ...defaultState, ...settings };
        activeSection = 'compression';
        if (open) {
            ffmpegEncodersStore.load();
        }
    }

    let encodersUnsub: () => void;
    let ffmpegEncodersState = { encoders: [], loading: false, error: null } as any;
    encodersUnsub = ffmpegEncodersStore.subscribe(state => (ffmpegEncodersState = state));
    onDestroy(() => encodersUnsub());

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

    function handleAppScaleChange(event: Event) {
        const value = parseFloat((event.target as HTMLInputElement).value);
        if (!Number.isNaN(value)) {
            localSettings = { ...localSettings, appScale: Math.max(0.5, Math.min(2.0, value)) };
        }
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

                <div class="modal-content">
                    <!-- Sidebar Navigation -->
                    <nav class="settings-sidebar">
                        {#each sections as section}
                            <button
                                type="button"
                                class="sidebar-item {activeSection === section.id ? 'active' : ''}"
                                on:click={() => (activeSection = section.id)}
                            >
                                <span class="sidebar-icon">{section.icon}</span>
                                <span class="sidebar-label">{section.label}</span>
                            </button>
                        {/each}
                    </nav>

                    <!-- Main Content Area -->
                    <div class="settings-main">
                        {#if activeSection === 'compression'}
                            <div class="section-content">
                                <h3 class="section-title">Compression Settings</h3>
                                <p class="section-description">Configure default compression behavior for new videos.</p>

                                <div class="settings-grid">
                                    <div class="form-group">
                                        <label for="defaultCodec">default codec</label>
                                        <select id="defaultCodec" value={localSettings.defaultCodec} on:change={handleCodecChange}>
                                            <option value="fast">fast (h.264)</option>
                                            <option value="quality">quality (h.265)</option>
                                        </select>
                                        <div class="helper-text">H.265 provides better compression but may be slower on some devices.</div>
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
                                        <div class="helper-text">Auto mode selects the best resolution based on target file size.</div>
                                    </div>

                                    <div class="form-group full-width">
                                        <!-- svelte-ignore a11y_label_has_associated_control -->
                                        <label>default compression target</label>
                                        <div class="slider-container">
                                            <input
                                                type="range"
                                                min="1"
                                                max="100"
                                                step="1"
                                                value={localSettings.defaultTargetSizeMb}
                                                on:input={handleTargetMbChange}
                                            />
                                            <span class="slider-value">{localSettings.defaultTargetSizeMb} MB</span>
                                        </div>
                                        <div class="helper-text">Default target file size for compression.</div>
                                    </div>

                                    <div class="form-group toggle full-width">
                                        <label>
                                            <input type="checkbox" checked={localSettings.defaultMuteAudio} on:change={handleMuteToggle} />
                                            mute audio by default
                                        </label>
                                        <div class="helper-text">Remove audio track from compressed videos.</div>
                                    </div>
                                </div>
                            </div>
                        {:else if activeSection === 'appearance'}
                            <div class="section-content">
                                <h3 class="section-title">Appearance</h3>
                                <p class="section-description">Customize the look and feel of the application.</p>

                                <div class="settings-grid">
                                    <div class="form-group full-width">
                                        <!-- svelte-ignore a11y_label_has_associated_control -->
                                        <label>app scale</label>
                                        <div class="slider-container">
                                            <input
                                                type="range"
                                                min="0.5"
                                                max="2"
                                                step="0.1"
                                                value={localSettings.appScale}
                                                on:input={handleAppScaleChange}
                                            />
                                            <span class="slider-value">{(localSettings.appScale * 100).toFixed(0)}%</span>
                                        </div>
                                        <div class="helper-text">Scale the entire UI (50% - 200%). Changes apply after saving.</div>
                                    </div>

                                    <div class="scale-preview full-width">
                                        <div class="preview-label">Preview</div>
                                        <div class="preview-box" style="transform: scale({localSettings.appScale}); transform-origin: top left;">
                                            <div class="preview-content">
                                                <span class="preview-icon">📹</span>
                                                <span class="preview-text">Sample UI Element</span>
                                            </div>
                                        </div>
                                    </div>
                                </div>
                            </div>
                        {:else if activeSection === 'system'}
                            <div class="section-content">
                                <h3 class="section-title">System</h3>
                                <p class="section-description">System settings and encoder information.</p>

                                <div class="settings-grid">
                                    <div class="form-group toggle full-width">
                                        <label>
                                            <input
                                                type="checkbox"
                                                checked={localSettings.checkForUpdatesOnLaunch}
                                                on:change={handleUpdateToggle}
                                            />
                                            check for updates on launch
                                        </label>
                                        <div class="helper-text">Automatically check for new versions when the app starts.</div>
                                    </div>

                                    <div class="form-group full-width">
                                        <div class="setting-label"><strong>available encoders</strong></div>
                                        {#if ffmpegEncodersState.loading}
                                            <div class="encoder-loading">loading encoders...</div>
                                        {:else if ffmpegEncodersState.error}
                                            <div class="helper-text">{ffmpegEncodersState.error} <button type="button" class="inline-btn" on:click={() => ffmpegEncodersStore.refresh()}>retry</button></div>
                                        {:else if ffmpegEncodersState.encoders.length === 0}
                                            <div class="helper-text">No encoders found on this system</div>
                                        {:else}
                                            <div class="encoder-tags-wrapper">
                                                <div class="encoder-tags">
                                                    {#each ffmpegEncodersState.encoders as enc}
                                                        <span class="encoder-tag {enc.isAvailable === false ? 'muted' : ''}" title={enc.description}>
                                                            {enc.name}
                                                            {#if enc.isHardware}
                                                                <span class="hw-badge">HW</span>
                                                            {/if}
                                                        </span>
                                                    {/each}
                                                </div>
                                                <div class="encoder-action-row">
                                                    <button type="button" class="action-btn primary verify-button" on:click={() => ffmpegEncodersStore.refresh(true)} aria-label="Verify encoders">Verify Encoders</button>
                                                </div>
                                            </div>
                                        {/if}
                                        <div class="helper-text">Hardware encoders (HW) provide faster compression when available.</div>
                                    </div>
                                </div>
                            </div>
                        {/if}
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
        background: rgba(0, 0, 0, 0.7);
        display: flex;
        align-items: center;
        justify-content: center;
        z-index: 200;
        padding: 24px;
    }

    .settings-modal {
        width: calc(100vw - 48px);
        max-width: 900px;
        height: calc(100vh - 48px);
        max-height: 700px;
        background: #0f0f14;
        border: 1px solid #27272a;
        border-radius: 8px;
        box-shadow: 0 25px 80px rgba(0, 0, 0, 0.6);
        animation: fadeIn 0.2s ease-out;
        overflow: hidden;
        display: flex;
        flex-direction: column;
    }

    form {
        display: flex;
        flex-direction: column;
        flex: 1 1 auto;
        min-height: 0;
        overflow: hidden;
    }

    .modal-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 20px 24px;
        border-bottom: 1px solid #27272a;
        flex-shrink: 0;
    }

    .modal-header h2 {
        margin: 0;
        font-size: 1.1rem;
        text-transform: lowercase;
        color: #fafafa;
    }

    .icon-button {
        border: 1px solid #27272a;
        background: transparent;
        color: #fafafa;
        border-radius: 4px;
        width: 32px;
        height: 32px;
        cursor: pointer;
        font-size: 1rem;
        transition: background 0.15s, border-color 0.15s;
    }

    .icon-button:hover {
        background: rgba(255, 255, 255, 0.05);
        border-color: #3f3f46;
    }

    .modal-content {
        display: flex;
        flex: 1 1 auto;
        min-height: 0;
        overflow: hidden;
    }

    /* Sidebar Navigation */
    .settings-sidebar {
        width: 200px;
        min-width: 200px;
        background: rgba(24, 24, 27, 0.5);
        border-right: 1px solid #27272a;
        padding: 16px 12px;
        display: flex;
        flex-direction: column;
        gap: 4px;
        flex-shrink: 0;
    }

    .sidebar-item {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 12px 16px;
        border: none;
        background: transparent;
        color: #a1a1aa;
        font-size: 0.9rem;
        text-align: left;
        border-radius: 6px;
        cursor: pointer;
        transition: background 0.15s, color 0.15s;
    }

    .sidebar-item:hover {
        background: rgba(255, 255, 255, 0.05);
        color: #fafafa;
    }

    .sidebar-item.active {
        background: rgba(34, 211, 238, 0.1);
        color: #22d3ee;
    }

    .sidebar-icon {
        font-size: 1.1rem;
        width: 24px;
        text-align: center;
    }

    .sidebar-label {
        font-weight: 500;
    }

    /* Main Content Area */
    .settings-main {
        flex: 1 1 auto;
        overflow-y: auto;
        padding: 24px 32px;
        min-height: 0;
    }

    .section-content {
        max-width: 600px;
    }

    .section-title {
        margin: 0 0 8px 0;
        font-size: 1.25rem;
        font-weight: 600;
        color: #fafafa;
    }

    .section-description {
        margin: 0 0 24px 0;
        font-size: 0.9rem;
        color: #71717a;
    }

    .settings-grid {
        display: grid;
        grid-template-columns: repeat(2, 1fr);
        gap: 24px;
    }

    .form-group {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .form-group.full-width {
        grid-column: 1 / -1;
    }

    .form-group label {
        font-size: 0.85rem;
        color: #a1a1aa;
        text-transform: lowercase;
    }

    select {
        width: 100%;
        padding: 12px 14px;
        border-radius: 6px;
        border: 1px solid #27272a;
        background: rgba(24, 24, 27, 0.8);
        color: #fafafa;
        font-size: 0.9rem;
        transition: border-color 0.15s;
    }

    select:focus {
        outline: none;
        border-color: #22d3ee;
    }

    .slider-container {
        display: flex;
        align-items: center;
        gap: 16px;
    }

    .slider-container input[type='range'] {
        flex: 1;
        accent-color: #22d3ee;
    }

    .slider-value {
        min-width: 60px;
        text-align: right;
        font-size: 0.9rem;
        font-weight: 600;
        color: #22d3ee;
    }

    .helper-text {
        font-size: 0.75rem;
        color: #71717a;
        line-height: 1.4;
    }

    .form-group.toggle label {
        display: inline-flex;
        align-items: center;
        gap: 10px;
        color: #fafafa;
        font-size: 0.9rem;
        text-transform: lowercase;
        cursor: pointer;
    }

    .form-group.toggle input[type='checkbox'] {
        width: 18px;
        height: 18px;
        accent-color: #22d3ee;
    }

    /* Scale Preview */
    .scale-preview {
        margin-top: 8px;
    }

    .preview-label {
        font-size: 0.75rem;
        color: #71717a;
        margin-bottom: 12px;
        text-transform: uppercase;
        letter-spacing: 0.05em;
    }

    .preview-box {
        background: rgba(24, 24, 27, 0.6);
        border: 1px solid #27272a;
        border-radius: 8px;
        padding: 16px;
        display: inline-block;
        transition: transform 0.2s ease;
    }

    .preview-content {
        display: flex;
        align-items: center;
        gap: 12px;
        color: #fafafa;
    }

    .preview-icon {
        font-size: 1.5rem;
    }

    .preview-text {
        font-size: 0.9rem;
        font-weight: 500;
    }

    /* Encoder Tags */
    .setting-label {
        font-size: 0.85rem;
        color: #a1a1aa;
        margin-bottom: 4px;
    }

    .encoder-loading {
        color: #71717a;
        font-size: 0.85rem;
        padding: 8px 0;
    }

    .encoder-tags-wrapper {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    .encoder-tags {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
        padding: 8px 0;
    }

    .encoder-tag {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        padding: 8px 12px;
        border-radius: 999px;
        background: rgba(39, 39, 42, 0.6);
        border: 1px solid #3f3f46;
        color: #fafafa;
        font-size: 0.85rem;
        font-weight: 500;
    }

    .encoder-tag.muted {
        opacity: 0.5;
        filter: grayscale(0.3);
    }

    .hw-badge {
        display: inline-block;
        padding: 2px 6px;
        border-radius: 999px;
        background: rgba(34, 211, 238, 0.15);
        color: #22d3ee;
        font-weight: 700;
        font-size: 0.7rem;
        border: 1px solid rgba(34, 211, 238, 0.1);
    }

    .encoder-action-row {
        display: flex;
        gap: 8px;
    }

    .verify-button {
        padding: 10px 20px;
        border-radius: 6px;
        font-weight: 600;
        font-size: 0.85rem;
    }

    .inline-btn {
        background: transparent;
        border: none;
        color: #22d3ee;
        cursor: pointer;
        text-decoration: underline;
        font-size: inherit;
    }

    .action-btn.primary {
        background: #22d3ee;
        color: #0f0f14;
        border: none;
        cursor: pointer;
        transition: background 0.15s;
    }

    .action-btn.primary:hover {
        background: #06b6d4;
    }

    /* Footer */
    .modal-footer {
        display: flex;
        justify-content: flex-end;
        gap: 12px;
        padding: 16px 24px;
        border-top: 1px solid #27272a;
        flex-shrink: 0;
    }

    .modal-footer button {
        padding: 10px 20px;
        border-radius: 6px;
        border: 1px solid #27272a;
        cursor: pointer;
        text-transform: lowercase;
        font-size: 0.9rem;
        font-weight: 500;
        transition: background 0.15s, border-color 0.15s;
    }

    .modal-footer .secondary {
        background: transparent;
        color: #a1a1aa;
    }

    .modal-footer .secondary:hover {
        background: rgba(255, 255, 255, 0.05);
        color: #fafafa;
    }

    .modal-footer .primary {
        background: #22d3ee;
        color: #0f0f14;
        border-color: #22d3ee;
        font-weight: 600;
    }

    .modal-footer .primary:hover {
        background: #06b6d4;
        border-color: #06b6d4;
    }

    @keyframes fadeIn {
        from {
            opacity: 0;
            transform: scale(0.98);
        }
        to {
            opacity: 1;
            transform: scale(1);
        }
    }

    /* Responsive adjustments */
    @media (max-width: 700px) {
        .settings-sidebar {
            width: 60px;
            min-width: 60px;
            padding: 16px 8px;
        }

        .sidebar-label {
            display: none;
        }

        .sidebar-item {
            justify-content: center;
            padding: 12px;
        }

        .sidebar-icon {
            margin: 0;
        }

        .settings-grid {
            grid-template-columns: 1fr;
        }

        .settings-main {
            padding: 20px;
        }
    }
</style>
