<script lang="ts">
    import OutputControls from '../OutputControls.svelte';
    import ActionButtons from '../ActionButtons.svelte';
    import AdvancedOptions from '../AdvancedOptions.svelte';

    export let metadataVisible = false;
    export let metadataContent = '';
    export let controlsVisible = false;
    export let outputSizeValue = '--';
    export let outputSizeDetails = '';
    export let outputSizeSliderValue = 100;
    export let outputSizeSliderDisabled = true;
    export let sliderMax = 100;
    export let sliderStep = 1;
    export let codecSelectValue = 'quality';
    export let codecHelperText = '';
    export let uploadBtnDisabled = true;
    export let uploadBtnText = 'Process Video';
    export let showCancelButton = false;
    export let muteAudio = false;
    export let resolutionPreset = 'auto';

    export let onPresetClick: (value: string) => void = () => {};
    export let onSliderChange: (value: number) => void = () => {};
    export let onCodecChange: (value: string) => void = () => {};
    export let onUploadClick: (event: MouseEvent) => void = () => {};
    export let onCancelClick: () => void = () => {};
    export let onMuteToggle: (value: boolean) => void = () => {};
    export let onResolutionChange: (value: string) => void = () => {};
</script>

<aside class="sidebar">
    <div class="sidebar-content">
        {#if metadataVisible}
            <section class="sidebar-section">
                <h2 class="section-title">// file_info</h2>
                <div class="metadata">
                    {@html metadataContent}
                </div>
            </section>
        {/if}

        {#if controlsVisible}
            <section class="sidebar-section">
                <h2 class="section-title">// settings</h2>
                <OutputControls
                    {outputSizeValue}
                    {outputSizeDetails}
                    {outputSizeSliderValue}
                    {outputSizeSliderDisabled}
                    {sliderMax}
                    {sliderStep}
                    codecSelectValue={codecSelectValue}
                    codecHelperText={codecHelperText}
                    muteAudio={muteAudio}
                    resolutionPreset={resolutionPreset}
                    on:presetClick={(e) => onPresetClick(e.detail)}
                    on:sliderChange={(e) => onSliderChange(e.detail)}
                    on:codecChange={(e) => onCodecChange(e.detail)}
                    on:resolutionChange={(e) => onResolutionChange(e.detail)}
                    on:muteToggle={(e) => onMuteToggle(e.detail)}
                />

                <AdvancedOptions>
                    <!-- no advanced options yet; slot placeholder -->
                </AdvancedOptions>
            </section>

            <section class="sidebar-section">
                <ActionButtons
                    uploadBtnDisabled={uploadBtnDisabled}
                    uploadBtnText={uploadBtnText}
                    showCancelButton={showCancelButton}
                    on:uploadClick={(e) => onUploadClick(e.detail)}
                    on:cancelClick={() => onCancelClick()}
                />
            </section>
        {/if}
    </div>
</aside>

