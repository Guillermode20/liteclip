<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import type { UpdateInfoPayload } from '../types';
    const dispatch = createEventDispatcher();
    export let updateInfo: UpdateInfoPayload | null = null;
    export let showUpdateBanner = false;
    const openSettings = () => dispatch('openSettings');
    const dismiss = () => dispatch('dismissUpdate');
    function handleLogoError(e: Event) {
        const img = e?.target as HTMLImageElement | null;
        if (img) img.src = '/assets/logo.svg';
    }
</script>

<header class="app-header">
    <div class="header-title">
        <img class="app-logo" src="/logo.svg" alt="LiteClip logo" on:error={handleLogoError} />
        <h1>liteclip</h1>
        {#if updateInfo}
            <span class="version-chip">v{updateInfo.currentVersion}</span>
        {/if}
    </div>
    <div class="header-actions">
        <button class="icon-btn" type="button" on:click={() => openSettings()}>
            ⚙ settings
        </button>
    </div>
    {#if showUpdateBanner && updateInfo?.updateAvailable}
        <div class="update-banner">
            <span>
                New version <strong>{updateInfo.latestVersion}</strong> is available.
            </span>
            <a
                class="update-link"
                href={updateInfo.downloadUrl || 'https://github.com/Guillermode20/smart-compressor/releases'}
                target="_blank"
                rel="noreferrer"
            >
                download
            </a>
            <button type="button" class="dismiss-btn" on:click={dismiss}>
                dismiss
            </button>
        </div>
    {/if}
</header>
