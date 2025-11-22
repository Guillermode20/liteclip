<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import { videoStore } from '../stores/video';

    const dispatch = createEventDispatcher<{ fileSelected: { file: File } }>();

    export let selectedFile: File | null = null;
    export let hasControls = false;
    export let fileInfo = '';

    let isDragover = false;
    let fileInputRef: HTMLInputElement | null = null;

    function triggerFileInput() {
        fileInputRef?.click();
    }

    function handleFileSelection(files: FileList | null) {
        if (files && files.length > 0) {
            const file = files[0];
            videoStore.setFile(file);
            dispatch('fileSelected', { file });
        }
    }

    function handleDrop(event: DragEvent) {
        event.preventDefault();
        isDragover = false;
        handleFileSelection(event.dataTransfer?.files ?? null);
    }

    function handleDragOver(event: DragEvent) {
        event.preventDefault();
        isDragover = true;
    }

    function handleDragLeave() {
        isDragover = false;
    }

    function handleFileInputChange(event: Event) {
        const target = event.target as HTMLInputElement;
        handleFileSelection(target.files);
    }

    $: if (!selectedFile && fileInputRef) {
        fileInputRef.value = '';
    }

    $: shouldShowReadyState = Boolean(selectedFile && hasControls);
</script>

<div class="content-card">
    <h2 class="section-title">// upload_video</h2>
    <div 
        class="upload-area" 
        class:dragover={isDragover}
        class:has-video={shouldShowReadyState}
        on:dragover={handleDragOver}
        on:dragleave={handleDragLeave}
        on:drop={handleDrop}
        role="region"
        aria-label="Video upload area"
    >
        <input 
            type="file" 
            accept="video/*" 
            style="display: none;"
            bind:this={fileInputRef}
            on:change={handleFileInputChange}
        />

        {#if shouldShowReadyState}
            <div 
                class="upload-state"
                on:click={triggerFileInput}
                on:keydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); triggerFileInput(); } }}
                role="button"
                tabindex="0"
            >
                <div class="upload-state-text">
                    <p class="upload-state-title">Video ready for editing</p>
                    <p class="upload-state-helper">Drop another file to replace it</p>
                </div>
                {#if fileInfo}
                    <div class="file-info">{fileInfo}</div>
                {/if}
            </div>
        {:else}
            <div 
                class="upload-prompt"
                on:click={triggerFileInput}
                on:keydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); triggerFileInput(); } }}
                role="button"
                tabindex="0"
            >
                <svg class="upload-icon" xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
                    <polyline points="17 8 12 3 7 8"></polyline>
                    <line x1="12" y1="3" x2="12" y2="15"></line>
                </svg>
                <p class="upload-text">Drop video or click to select</p>
            </div>
        {/if}
    </div>
</div>

