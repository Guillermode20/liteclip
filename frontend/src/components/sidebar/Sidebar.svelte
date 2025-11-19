<script lang="ts">
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

    function handleSliderInput(event: Event) {
        const value = parseFloat((event.target as HTMLInputElement).value);
        onSliderChange(value);
    }

    function handleCodecSelect(event: Event) {
        const value = (event.target as HTMLSelectElement).value;
        onCodecChange(value);
    }

    function handleResolutionSelect(event: Event) {
        const value = (event.target as HTMLSelectElement).value;
        onResolutionChange(value);
    }

    function handleMuteChange(event: Event) {
        const checked = (event.target as HTMLInputElement).checked;
        onMuteToggle(checked);
    }
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
                <div class="settings-group">
                    <label for="outputSizeSlider" class="setting-label">
                        <strong>target_size</strong>
                        <span class="setting-value">{outputSizeValue}</span>
                    </label>
                    <div class="preset-buttons">
                        <button type="button" class="preset-btn" on:click={() => onPresetClick('25')}>-75%</button>
                        <button type="button" class="preset-btn" on:click={() => onPresetClick('50')}>-50%</button>
                        <button type="button" class="preset-btn" on:click={() => onPresetClick('75')}>-25%</button>
                    </div>
                    <input 
                        type="range" 
                        id="outputSizeSlider" 
                        min="1" 
                        max={sliderMax} 
                        step={sliderStep}
                        value={outputSizeSliderValue}
                        disabled={outputSizeSliderDisabled}
                        on:input={handleSliderInput}
                        class="size-slider"
                    />
                    <div class="helper-text">// drag to adjust compression</div>
                    {#if outputSizeDetails}
                        <div class="estimate-line">→ {outputSizeDetails}</div>
                    {/if}
                </div>

                <div class="settings-group">
                    <label for="codecSelect" class="setting-label">
                        <strong>codec</strong>
                    </label>
                    <select id="codecSelect" value={codecSelectValue} on:change={handleCodecSelect}>
                        <option value="fast">fast (h.264)</option>
                        <option value="quality">quality (h.265)</option>
                    </select>
                    {#if codecHelperText}
                        <div class="helper-text">// {codecHelperText}</div>
                    {/if}
                </div>

                <div class="settings-group">
                    <label for="resolutionSelect" class="setting-label">
                        <strong>resolution</strong>
                    </label>
                    <select id="resolutionSelect" value={resolutionPreset} on:change={handleResolutionSelect}>
                        <option value="auto">auto (smart)</option>
                        <option value="source">original</option>
                        <option value="1080p">1080p</option>
                        <option value="720p">720p</option>
                        <option value="480p">480p</option>
                        <option value="360p">360p</option>
                    </select>
                    <div class="helper-text">// force target resolution if needed</div>
                </div>

                <div class="settings-group toggle-group">
                    <label class="toggle">
                        <input type="checkbox" checked={muteAudio} on:change={handleMuteChange} />
                        <span>mute audio</span>
                    </label>
                    <div class="helper-text">// turn sound off to save space</div>
                </div>
            </section>

            <section class="sidebar-section">
                <button 
                    id="uploadBtn" 
                    disabled={uploadBtnDisabled}
                    on:click={onUploadClick}
                    class="action-btn primary"
                >
                    $ {uploadBtnText.toLowerCase().replace('&', '+')}
                </button>

                {#if showCancelButton}
                    <button 
                        id="cancelBtn"
                        on:click={onCancelClick}
                        class="action-btn danger"
                    >
                        $ cancel processing
                    </button>
                {/if}
            </section>
        {/if}
    </div>
</aside>

