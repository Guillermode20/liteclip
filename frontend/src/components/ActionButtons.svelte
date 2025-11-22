<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    const dispatch = createEventDispatcher();

    export let uploadBtnDisabled = true;
    export let uploadBtnText = 'Process Video';
    export let showCancelButton = false;

    function handleUploadClick(event: MouseEvent) {
        dispatch('uploadClick', event);
    }

    function handleCancelClick() {
        dispatch('cancelClick');
    }
</script>

<div class="action-buttons">
    <button id="uploadBtn" disabled={uploadBtnDisabled} on:click={handleUploadClick} class="action-btn primary">
        $ {uploadBtnText.toLowerCase().replace('&', '+')}
    </button>

    {#if showCancelButton}
        <button id="cancelBtn" on:click={handleCancelClick} class="action-btn danger">
            $ cancel processing
        </button>
    {/if}
</div>
