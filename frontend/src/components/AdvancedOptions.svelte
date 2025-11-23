<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    const dispatch = createEventDispatcher();
    export let expanded = false;

    function toggle() {
        expanded = !expanded;
        dispatch('toggle', { expanded });
    }
</script>