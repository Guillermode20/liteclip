<script lang="ts">
    export let metadataVisible = false;
    export let metadataContent = '';
    export let controlsVisible = false;
    export let outputSizeValue = '--';
    export let outputSizeDetails = '';
    export let outputSizeSliderValue = 100;
    export let outputSizeSliderDisabled = true;
    export let codecSelectValue = 'h265';
    export let codecHelperText = '';
    export let uploadBtnDisabled = true;
    export let uploadBtnText = 'Process Video';
    export let showCancelButton = false;

    export let onPresetClick: (value: string) => void = () => {};
    export let onSliderChange: (value: number) => void = () => {};
    export let onCodecChange: (value: string) => void = () => {};
    export let onUploadClick: (event: MouseEvent) => void = () => {};
    export let onCancelClick: () => void = () => {};

    function handleSliderInput(event: Event) {
        const value = parseFloat((event.target as HTMLInputElement).value);
        onSliderChange(value);
    }

    function handleCodecSelect(event: Event) {
        const value = (event.target as HTMLSelectElement).value;
        onCodecChange(value);
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
                        max="100" 
                        step="0.5"
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
                        <option value="h264">h264 (mp4)</option>
                        <option value="h265">h265 / hevc (mp4)</option>
                    </select>
                    {#if codecHelperText}
                        <div class="helper-text">// {codecHelperText}</div>
                    {/if}
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

