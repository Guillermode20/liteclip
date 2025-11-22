<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    const dispatch = createEventDispatcher();
    export let expanded = false;

    function toggle() {
        expanded = !expanded;
        dispatch('toggle', { expanded });
    }
</script>

<div class="advanced-options">
    <button class="advanced-toggle" on:click={toggle} aria-expanded={expanded}>
        {#if expanded}Hide Advanced Options{:else}Show Advanced Options{/if}
    </button>
    {#if expanded}
        <div class="advanced-content">
            <slot />
        </div>
    {/if}
</div>
